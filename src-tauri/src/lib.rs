pub mod mikrofon;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};

/// Açık mikrofonu kapatmak için tutulan kol. Düşürülünce (drop) mikrofon thread'i
/// biter ve mikrofon kapanır.
struct MikrofonKolu {
    _dur: mpsc::Sender<()>,
}

#[derive(Default)]
struct Durum {
    mikrofon: Mutex<Option<MikrofonKolu>>,
}

#[derive(Serialize)]
struct MikrofonBilgisi {
    cihaz: String,
    hiz: u32,
}

/// Mikrofonu açar ve her 10 ms'lik kareyi `kanal` üzerinden arayüze yollar.
/// Kare: 480 örnek, 48 kHz, mono, 16-bit (little-endian) = 960 bayt.
/// 16-bit seçildi çünkü 1 KB altı mesajları Tauri doğrudan (fetch olmadan) iletiyor;
/// konuşma için 16-bit kalite kaybı yok (Opus zaten bundan daha düşük çözünürlükte kodlar).
///
/// `async`: Tauri'de async olmayan komutlar ana (arayüz) thread'inde çalışır. Mikrofonun
/// açılmasını orada beklemek arayüzü kilitliyor ve Channel mesajlarını (onlar da ana
/// thread'den geçer) saniyelerce bekletiyordu. async komut arka plan havuzunda çalışır.
#[tauri::command]
async fn mikrofon_ac(
    durum: tauri::State<'_, Durum>,
    kanal: Channel<InvokeResponseBody>,
) -> Result<MikrofonBilgisi, String> {
    // Önceki mikrofon varsa önce onu kapat (aynı anda iki mikrofon akışı olmasın).
    *durum.mikrofon.lock().unwrap() = None;

    let (dur_tx, dur_rx) = mpsc::channel::<()>();
    let (hazir_tx, hazir_rx) = mpsc::channel();

    // cpal akışı bazı sistemlerde thread'ler arası taşınamıyor (Send değil); bu yüzden
    // mikrofon kendi thread'inde yaşar, kapatma sinyali gelene kadar bekler.
    thread::Builder::new()
        .name("mikrofon".into())
        .spawn(move || {
            let mut bayt = vec![0u8; mikrofon::KARE * 2];
            let sonuc = mikrofon::Mikrofon::ac(Arc::new(AtomicBool::new(true)), move |kare, _| {
                for (i, &s) in kare.iter().enumerate() {
                    let v = (s * 32767.0).round() as i16;
                    bayt[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
                }
                // Arayüz kapandıysa gönderim hata verir; mikrofon kapatılana kadar sessizce geçilir.
                let _ = kanal.send(InvokeResponseBody::Raw(bayt.clone()));
            });
            match sonuc {
                Ok(mik) => {
                    let _ = hazir_tx.send(Ok(MikrofonBilgisi {
                        cihaz: mik.bilgi.cihaz.clone(),
                        hiz: mik.bilgi.cihaz_hizi,
                    }));
                    // Gönderen taraf düşürülünce recv hata döner ve thread biter.
                    let _ = dur_rx.recv();
                    drop(mik);
                }
                Err(e) => {
                    let _ = hazir_tx.send(Err(e.to_string()));
                }
            }
        })
        .map_err(|e| e.to_string())?;

    // Bekleme bloklayan bir işlem; async havuzu tıkamasın diye ayrı thread'de beklenir.
    let bilgi = tauri::async_runtime::spawn_blocking(move || hazir_rx.recv())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())??;
    *durum.mikrofon.lock().unwrap() = Some(MikrofonKolu { _dur: dur_tx });
    Ok(bilgi)
}

#[tauri::command]
fn mikrofon_kapat(durum: tauri::State<'_, Durum>) {
    *durum.mikrofon.lock().unwrap() = None;
}

/// Sadece geliştirme sürümünde kullanılır: sayfadaki konsol mesajlarını terminale yazar.
/// (Sürüm derlemesinde sayfaya bağlayan betik eklenmediği için hiç çağrılmaz.)
#[tauri::command]
fn konsol(seviye: String, mesaj: String) {
    eprintln!("[sayfa {seviye}] {mesaj}");
}

/// Geliştirme sürümünde sayfa hatalarını, konsolu ve WebSocket olaylarını terminale aktarır.
/// Mac'te WKWebView'in konsolu dışarıdan görünmediği için hata ayıklamada gerekli.
#[cfg(debug_assertions)]
const KONSOL_BETIGI: &str = r#"
(() => {
  const yolla = (seviye, ...a) => {
    try {
      const mesaj = a.map(x => x instanceof Error ? x.stack || String(x)
        : typeof x === 'object' ? JSON.stringify(x) : String(x)).join(' ')
      window.__TAURI_INTERNALS__.invoke('konsol', {seviye, mesaj})
    } catch {}
  }
  for (const s of ['log', 'warn', 'error']) {
    const asil = console[s].bind(console)
    console[s] = (...a) => { asil(...a); yolla(s, ...a) }
  }
  addEventListener('error', e => yolla('hata', e.message, e.filename + ':' + e.lineno))
  addEventListener('unhandledrejection', e => yolla('promise', e.reason))
  addEventListener('securitypolicyviolation', e => yolla('csp', e.violatedDirective, e.blockedURI))
  const WS = window.WebSocket
  window.WebSocket = function (url, ...r) {
    const ws = new WS(url, ...r)
    ws.addEventListener('open', () => yolla('ws', 'açıldı', url))
    ws.addEventListener('error', () => yolla('ws', 'HATA', url))
    ws.addEventListener('close', e => yolla('ws', 'kapandı', url, e.code))
    return ws
  }
  window.WebSocket.prototype = WS.prototype
  Object.assign(window.WebSocket, {CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3})
  // Ses zinciri teşhisi: AudioContext durumu ve worklet'e giden paket sayısı.
  const AC = window.AudioContext
  window.AudioContext = function (...a) { const c = new AC(...a); window.__ctx = c; return c }
  window.AudioContext.prototype = AC.prototype
  let paket = 0
  const pm = MessagePort.prototype.postMessage
  MessagePort.prototype.postMessage = function (...a) { paket++; return pm.apply(this, a) }
  const PC = window.RTCPeerConnection, baglantilar = []
  window.RTCPeerConnection = function (...a) { const p = new PC(...a); baglantilar.push(p); return p }
  window.RTCPeerConnection.prototype = PC.prototype
  setInterval(async () => {
    const c = window.__ctx
    if (c) yolla('ses', 'ctx', c.state, 'zaman', c.currentTime.toFixed(2), 'hız', c.sampleRate, 'paket', paket)
    for (const p of baglantilar) {
      if (p.connectionState !== 'connected') continue
      const ozet = []
      ;(await p.getStats()).forEach(r => {
        if (r.type === 'media-source' && r.kind === 'audio') ozet.push('kaynak seviye=' + r.audioLevel + ' enerji=' + r.totalAudioEnergy)
        if (r.type === 'outbound-rtp' && r.kind === 'audio') ozet.push('giden paket=' + r.packetsSent)
        if (r.type === 'inbound-rtp' && r.kind === 'audio') ozet.push('gelen paket=' + r.packetsReceived + ' enerji=' + r.totalAudioEnergy)
      })
      yolla('rtc', ozet.join(' | '))
    }
  }, 2000)
  yolla('log', 'teşhis betiği yüklendi')
})();
"#;

pub fn calistir() {
    let builder = tauri::Builder::default();
    #[cfg(debug_assertions)]
    let builder = builder.on_page_load(|webview, yukleme| {
        if matches!(yukleme.event(), tauri::webview::PageLoadEvent::Finished) {
            let _ = webview.eval(KONSOL_BETIGI);
            // Otomatik test: SESAPP_TEST_BETIK verilirse o JS dosyası sayfada çalıştırılır
            // (giriş yapma, mesaj yazma, ekranı okuyup console.log ile terminale yazma).
            if let Ok(yol) = std::env::var("SESAPP_TEST_BETIK") {
                if let Ok(betik) = std::fs::read_to_string(yol) {
                    let _ = webview.eval(betik);
                }
            }
        }
    });
    builder
        .manage(Durum::default())
        .invoke_handler(tauri::generate_handler![mikrofon_ac, mikrofon_kapat, konsol])
        .run(tauri::generate_context!())
        .expect("uygulama başlatılamadı");
}

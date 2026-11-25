mod bildirim;
mod link;
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

struct Durum {
    mikrofon: Mutex<Option<MikrofonKolu>>,
    /// Gürültü engelleme (RNNoise) açık mı. Açık mikrofonda da anında etkili olur.
    gurultu: Arc<AtomicBool>,
}

impl Default for Durum {
    fn default() -> Self {
        Self { mikrofon: Mutex::new(None), gurultu: Arc::new(AtomicBool::new(true)) }
    }
}

/// Gürültü engellemeyi açar/kapatır (ayar arayüzde saklanır, açılışta buraya bildirilir).
#[tauri::command]
fn gurultu_engelle(durum: tauri::State<'_, Durum>, acik: bool) {
    durum.gurultu.store(acik, std::sync::atomic::Ordering::Relaxed);
}

#[derive(Serialize)]
struct MikrofonBilgisi {
    id: String,
    cihaz: String,
    hiz: u32,
}

#[derive(Serialize)]
struct MikrofonCihazi {
    id: String,
    ad: String,
    varsayilan: bool,
}

/// Sistemdeki mikrofonların listesi (menüde seçmek için). Hiçbirini açmaz.
/// async: Windows'ta cihazları saymak biraz sürebilir, arayüz thread'i beklemesin.
#[tauri::command]
async fn mikrofonlar() -> Result<Vec<MikrofonCihazi>, String> {
    let liste = mikrofon::cihazlar().map_err(|e| e.to_string())?;
    Ok(liste
        .into_iter()
        .map(|c| MikrofonCihazi { id: c.id, ad: c.ad, varsayilan: c.varsayilan })
        .collect())
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
    cihaz: Option<String>,
) -> Result<MikrofonBilgisi, String> {
    // Önceki mikrofon varsa önce onu kapat (aynı anda iki mikrofon akışı olmasın).
    *durum.mikrofon.lock().unwrap() = None;
    let gurultu = durum.gurultu.clone();

    let (dur_tx, dur_rx) = mpsc::channel::<()>();
    let (hazir_tx, hazir_rx) = mpsc::channel();

    // cpal akışı bazı sistemlerde thread'ler arası taşınamıyor (Send değil); bu yüzden
    // mikrofon kendi thread'inde yaşar, kapatma sinyali gelene kadar bekler.
    thread::Builder::new()
        .name("mikrofon".into())
        .spawn(move || {
            let mut bayt = vec![0u8; mikrofon::KARE * 2];
            let sonuc = mikrofon::Mikrofon::ac(cihaz.as_deref(), gurultu, move |kare, _| {
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
                        id: mik.bilgi.id.clone(),
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

/// Güncelleme denemeleri arasındaki bekleme (saniye): açılışta hemen, olmazsa 15 sn,
/// 1 dk ve 5 dk sonra. Mac'te yeni sürümün ilk açılışında tek kontrol bazen başarısız
/// oluyor (internet henüz hazır değil ya da sistem yeni uygulamayı tarıyor), ikinci
/// açılışta çalışıyordu. 5 dakikadan sonra denenmez ki oyun ortasında yeniden başlamasın.
const GUNCELLEME_BEKLEMELERI: [u64; 4] = [0, 15, 60, 300];

/// Açılışta GitHub'daki latest.json'a bakar; yeni sürüm varsa indirir, imzasını
/// (bizim gizli anahtarımızla imzalanmış mı) doğrular, kurar ve uygulamayı yeniden başlatır.
/// Windows'ta kurulum dosyası "passive" modda çalışır (sadece ilerleme çubuğu).
/// Her denemenin sonucu guncelleme.log'a yazılır (sorun olursa sebebi görülsün).
/// Geliştirme sürümünde çalışmaz, yoksa test sırasında kendini güncellerdi.
fn guncelleme_dongusu(app: tauri::AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    let yer = std::env::current_exe().map(|y| y.display().to_string()).unwrap_or_default();
    guncelleme_kaydi(&app, &format!("açıldı ({yer})"));
    for (no, bekle) in GUNCELLEME_BEKLEMELERI.iter().enumerate() {
        thread::sleep(std::time::Duration::from_secs(*bekle));
        match tauri::async_runtime::block_on(guncelle(&app)) {
            Ok(Some(yeni)) => {
                guncelleme_kaydi(&app, &format!("{yeni} kuruldu, yeniden başlatılıyor"));
                app.restart();
            }
            Ok(None) => {
                guncelleme_kaydi(&app, "güncel");
                return;
            }
            Err(e) => guncelleme_kaydi(&app, &format!("deneme {}: {e}", no + 1)),
        }
    }
}

/// Yeni sürüm varsa indirip kurar ve sürüm numarasını döner; yoksa None.
async fn guncelle(app: &tauri::AppHandle) -> tauri_plugin_updater::Result<Option<String>> {
    use tauri::Emitter;
    use tauri_plugin_updater::UpdaterExt;

    let Some(surum) = app.updater()?.check().await? else {
        return Ok(None);
    };
    guncelleme_kaydi(app, &format!("{} bulundu, indiriliyor", surum.version));
    // Arayüz "güncelleniyor" yazsın; kişi neden yeniden başladığını anlasın.
    let _ = app.emit("guncelleme", surum.version.clone());
    surum.download_and_install(|_, _| {}, || {}).await?;
    Ok(Some(surum.version))
}

/// Güncelleme günlüğü: Mac'te ~/Library/Logs/io.github.yigitfire.sesapp/guncelleme.log,
/// Windows'ta %LOCALAPPDATA%\io.github.yigitfire.sesapp\logs\guncelleme.log.
/// 100 KB'ı geçince baştan başlar. Kişisel bilgi yazılmaz.
fn guncelleme_kaydi(app: &tauri::AppHandle, satir: &str) {
    eprintln!("Güncelleme: {satir}");
    gunluge_yaz(app, "guncelleme.log", satir);
}

/// Ses günlüğü (ses.log, güncelleme günlüğüyle aynı klasörde): kimin sesinin gelip
/// gelmediği, bağlantı olayları ve kendini onarma adımları. "Bazen duymuyorum" gibi
/// ara sıra olan sorunlarda o anda ne olduğu sonradan görülsün diye.
/// Saydam mod: pencerenin arkası buzlu (bulanık) görünür. Ne kadar saydam olacağını sayfa
/// zemin renklerinin saydamlığıyla ayarlar; burası sadece arkadaki bulanıklığı açıp kapatır.
/// Mac'te sistemin en saydam koyu buzlu cam malzemesi (HUD), Windows'ta akrilik.
#[tauri::command]
fn saydam(pencere: tauri::WebviewWindow, acik: bool) {
    use tauri::window::{Effect, EffectState, EffectsBuilder};
    if !acik {
        let _ = pencere.set_effects(None);
        return;
    }
    #[cfg(target_os = "macos")]
    let efekt = EffectsBuilder::new().effect(Effect::HudWindow).state(EffectState::Active);
    #[cfg(not(target_os = "macos"))]
    let efekt = EffectsBuilder::new().effect(Effect::Acrylic).color(tauri::window::Color(17, 17, 17, 30));
    let _ = pencere.set_effects(efekt.build());
}

#[tauri::command]
fn ses_kaydi(app: tauri::AppHandle, satir: String) {
    gunluge_yaz(&app, "ses.log", &satir.chars().take(2000).collect::<String>());
}

fn gunluge_yaz(app: &tauri::AppHandle, ad: &str, satir: &str) {
    use std::io::Write;
    use tauri::Manager;

    let surum = &app.package_info().version;
    let Ok(klasor) = app.path().app_log_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&klasor);
    let yol = klasor.join(ad);
    if std::fs::metadata(&yol).is_ok_and(|m| m.len() > 100_000) {
        let _ = std::fs::remove_file(&yol);
    }
    if let Ok(mut dosya) = std::fs::OpenOptions::new().create(true).append(true).open(&yol) {
        let _ = writeln!(dosya, "{} UTC  v{surum}  {satir}", utc_zaman());
    }
}

/// Şu anki UTC zamanı "2026-11-22 14:05:09" biçiminde (tarih kütüphanesi eklememek için elle).
fn utc_zaman() -> String {
    let sn = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (gun, kalan) = (sn.div_euclid(86_400), sn.rem_euclid(86_400));
    // Günden takvim tarihine (Howard Hinnant'ın civil_from_days algoritması).
    let z = gun + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", kalan / 3600, kalan % 3600 / 60, kalan % 60)
}

pub fn calistir() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let app = app.handle().clone();
            // Bildirim penceresi açılamazsa uygulama yine çalışır (sadece bildirim çıkmaz).
            if let Err(hata) = bildirim::olustur(&app) {
                eprintln!("Bildirim penceresi açılamadı: {hata}");
            }
            // Ayrı thread: denemeler arasında beklerken uygulamayı tutmaz. İnternet yoksa veya
            // GitHub'a ulaşılamazsa uygulama çalışmaya devam eder.
            thread::spawn(move || guncelleme_dongusu(app));
            Ok(())
        });
    #[cfg(debug_assertions)]
    let builder = builder.on_page_load(|webview, yukleme| {
        if matches!(yukleme.event(), tauri::webview::PageLoadEvent::Finished) && webview.label() == "main" {
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
        // Bildirim penceresi hep açık durduğu için ana pencere kapanınca uygulama kendiliğinden
        // kapanmazdı: ana pencere kapanınca uygulamadan çıkılır.
        .on_window_event(|pencere, olay| {
            if pencere.label() == "main" && matches!(olay, tauri::WindowEvent::Destroyed) {
                tauri::Manager::app_handle(pencere).exit(0);
            }
        })
        .invoke_handler(tauri::generate_handler![link::link_onizle, link::link_ac, mikrofon_ac, mikrofon_kapat, mikrofonlar, gurultu_engelle, ses_kaydi, konsol, saydam,
            bildirim::bildirim_goster, bildirim::bildirim_ac, bildirim::bildirim_bekle, bildirim::bildirim_birak, bildirim::bildirim_tikla])
        .run(tauri::generate_context!())
        .expect("uygulama başlatılamadı");
}

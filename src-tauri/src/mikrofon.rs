//! Mikrofonu tarayıcı motoru yerine doğrudan işletim sisteminden alır.
//!
//! Neden: Tarayıcı motorları mikrofonu "görüşme" moduyla açıyor.
//! - Windows (WebView2/Chromium): yakalama akışına AudioCategory_Communications veriyor,
//!   Windows da "iletişim etkinliği" sayıp diğer sesleri (oyunu) kısıyor.
//! - Mac (WKWebView/WebKit): echoCancellation açıkken VoiceProcessingIO açıyor,
//!   bu da diğer uygulamaların sesini kısabiliyor.
//!
//! cpal ise Windows'ta mikrofonu eConsole rolüyle, kategori vermeden açar;
//! Mac'te düz HAL Output birimini kullanır (VoiceProcessingIO değil).
//! İkisinde de işletim sistemi bunu görüşme saymaz, ducking tetiklenmez.
//!
//! Akış: cpal geri çağrısı -> (mono, cihaz hızı) halka tampon -> işçi thread:
//! 48 kHz'e çevir -> RNNoise -> 480 örneklik (10 ms) karelerle dışarı ver.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use audioadapter_buffers::direct::InterleavedSlice;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use nnnoiseless::DenoiseState;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

/// Çıkış örnekleme hızı. RNNoise ve WebRTC/Opus için doğal hız.
pub const HIZ: u32 = 48_000;
/// Bir kare: 10 ms = 480 örnek. RNNoise bu boyutla çalışır.
pub const KARE: usize = nnnoiseless::FRAME_SIZE;

/// Açılan mikrofon hakkında bilgi.
#[derive(Debug, Clone)]
pub struct Bilgi {
    pub cihaz: String,
    pub cihaz_hizi: u32,
    pub kanal: u16,
}

/// Çalışan mikrofon. Düşürülünce (drop) mikrofon kapanır.
pub struct Mikrofon {
    pub bilgi: Bilgi,
    _akis: cpal::Stream,
    dur: Arc<AtomicBool>,
    isci: Option<JoinHandle<()>>,
}

impl Mikrofon {
    /// Varsayılan mikrofonu açar.
    ///
    /// `gurultu_engelle`: RNNoise açık mı (çalışırken değiştirilebilir).
    /// `cikis`: her 10 ms'de bir çağrılır; 480 örnek, 48 kHz, mono, [-1, 1] aralığında
    /// ve RNNoise'un konuşma olasılığı (0..1).
    pub fn ac<F>(gurultu_engelle: Arc<AtomicBool>, cikis: F) -> Result<Self>
    where
        F: FnMut(&[f32], f32) + Send + 'static,
    {
        let host = cpal::default_host();
        let cihaz = host
            .default_input_device()
            .ok_or_else(|| anyhow!("Mikrofon bulunamadı"))?;
        // Cihazın kendi varsayılan ayarını kullanıyoruz. Başka bir hız istemek cihazın
        // (örneğin USB kulaklığın) hızını değiştirip oyun sesini etkileyebilirdi.
        let ayar = cihaz
            .default_input_config()
            .context("Mikrofon ayarı okunamadı")?;
        let bilgi = Bilgi {
            cihaz: cihaz
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_else(|_| "Bilinmeyen".into()),
            cihaz_hizi: ayar.sample_rate(),
            kanal: ayar.channels(),
        };

        // ~1 saniyelik tampon: işçi kısa süre takılsa bile ses kaybolmaz.
        let (uretici, tuketici) = rtrb::RingBuffer::<f32>::new(bilgi.cihaz_hizi as usize);

        let dur = Arc::new(AtomicBool::new(false));
        let isci = {
            let dur = dur.clone();
            let cihaz_hizi = bilgi.cihaz_hizi;
            thread::Builder::new()
                .name("mikrofon-isci".into())
                .spawn(move || isci_dongusu(tuketici, cihaz_hizi, gurultu_engelle, dur, cikis))?
        };
        let isci_thread = isci.thread().clone();

        let kanal = bilgi.kanal as usize;
        let akis_ayari: cpal::StreamConfig = ayar.config();
        let akis = match ayar.sample_format() {
            SampleFormat::F32 => akis_kur::<f32>(&cihaz, &akis_ayari, kanal, uretici, isci_thread),
            SampleFormat::I16 => akis_kur::<i16>(&cihaz, &akis_ayari, kanal, uretici, isci_thread),
            SampleFormat::I32 => akis_kur::<i32>(&cihaz, &akis_ayari, kanal, uretici, isci_thread),
            SampleFormat::U16 => akis_kur::<u16>(&cihaz, &akis_ayari, kanal, uretici, isci_thread),
            diger => Err(anyhow!("Desteklenmeyen ses biçimi: {diger:?}")),
        }?;
        akis.play().context("Mikrofon başlatılamadı")?;

        Ok(Self {
            bilgi,
            _akis: akis,
            dur,
            isci: Some(isci),
        })
    }
}

impl Drop for Mikrofon {
    fn drop(&mut self) {
        self.dur.store(true, Ordering::Relaxed);
        if let Some(isci) = self.isci.take() {
            isci.thread().unpark();
            let _ = isci.join();
        }
    }
}

/// cpal giriş akışını kurar. Geri çağrı gerçek zamanlı ses thread'inde çalışır:
/// burada bellek ayırma, kilit veya ağır iş yok. Sadece kanalları monoya indirip
/// halka tampona yazar ve işçiyi uyandırır.
fn akis_kur<T>(
    cihaz: &cpal::Device,
    ayar: &cpal::StreamConfig,
    kanal: usize,
    mut uretici: rtrb::Producer<f32>,
    isci: thread::Thread,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let akis = cihaz.build_input_stream(
        *ayar,
        move |veri: &[T], _: &cpal::InputCallbackInfo| {
            for kare in veri.chunks_exact(kanal) {
                let mut toplam = 0.0f32;
                for &s in kare {
                    toplam += s.to_sample::<f32>();
                }
                // Tampon doluysa (işçi takıldıysa) örnek atılır; ses thread'i asla beklemez.
                let _ = uretici.push(toplam / kanal as f32);
            }
            isci.unpark();
        },
        |hata| eprintln!("Mikrofon hatası: {hata}"),
        None,
    )?;
    Ok(akis)
}

/// Halka tampondan okur, gerekiyorsa 48 kHz'e çevirir, RNNoise uygular, kare kare verir.
fn isci_dongusu<F>(
    mut tuketici: rtrb::Consumer<f32>,
    cihaz_hizi: u32,
    gurultu_engelle: Arc<AtomicBool>,
    dur: Arc<AtomicBool>,
    mut cikis: F,
) where
    F: FnMut(&[f32], f32),
{
    // Cihaz zaten 48 kHz ise hiç dönüştürme yapılmaz (en sık durum).
    let mut cevirici = (cihaz_hizi != HIZ).then(|| {
        let p = SincInterpolationParameters {
            // Konuşma için yeterli kalite, düşük işlemci ve düşük gecikme (~1,3 ms).
            sinc_len: 128,
            f_cutoff: None,
            oversampling_factor: 128,
            interpolation: SincInterpolationType::Cubic,
            window: WindowFunction::BlackmanHarris2,
        };
        // Çıkış sabit 480 örnek: her çağrı tam bir RNNoise karesi üretir.
        Async::<f32>::new_sinc(HIZ as f64 / cihaz_hizi as f64, 1.0, &p, KARE, 1, FixedAsync::Output)
            .expect("örnekleme dönüştürücü kurulamadı")
    });
    let giris_max = cevirici.as_ref().map_or(KARE, |c| c.input_frames_max());

    let mut giris = vec![0.0f32; giris_max];
    let mut kare = vec![0.0f32; KARE];
    let mut temiz = vec![0.0f32; KARE];
    let mut rnnoise = DenoiseState::new();

    while !dur.load(Ordering::Relaxed) {
        let gereken = cevirici.as_ref().map_or(KARE, |c| c.input_frames_next());
        if tuketici.slots() < gereken {
            // Veri gelince ses thread'i uyandırır; zaman aşımı sadece güvenlik için.
            thread::park_timeout(Duration::from_millis(50));
            continue;
        }
        let Ok(parca) = tuketici.read_chunk(gereken) else { continue };
        let (a, b) = parca.as_slices();
        giris[..a.len()].copy_from_slice(a);
        giris[a.len()..gereken].copy_from_slice(b);
        parca.commit_all();

        match cevirici.as_mut() {
            Some(c) => {
                let gir = InterleavedSlice::new(&giris[..gereken], 1, gereken).unwrap();
                let mut cik = InterleavedSlice::new_mut(&mut kare[..], 1, KARE).unwrap();
                if c.process_into_buffer(&gir, &mut cik, None).is_err() {
                    continue;
                }
            }
            None => kare.copy_from_slice(&giris[..KARE]),
        }

        if gurultu_engelle.load(Ordering::Relaxed) {
            // RNNoise 16-bit aralığında çalışır: [-1,1] -> [-32768,32767] ve geri.
            for s in kare.iter_mut() {
                *s *= 32768.0;
            }
            let konusma = rnnoise.process_frame(&mut temiz, &kare);
            for s in temiz.iter_mut() {
                *s = (*s / 32768.0).clamp(-1.0, 1.0);
            }
            cikis(&temiz, konusma);
        } else {
            cikis(&kare, 1.0);
        }
    }
}

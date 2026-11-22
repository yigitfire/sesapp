//! Mikrofon testi: varsayılan mikrofonu açar, RNNoise ile temizler,
//! her yarım saniyede seviye ve işlemci süresini yazar, sesi WAV olarak kaydeder.
//!
//! Çalıştırma: cargo run --release --example mikrofon_testi -- [saniye]

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sesapp::mikrofon::{HIZ, Mikrofon};

#[derive(Default)]
struct Olcum {
    kare: u64,
    tepe: f32,
    kare_toplam: f64,
    konusma_max: f32,
    son_kare_zamani: Option<Instant>,
    en_uzun_aralik: Duration,
}

fn main() -> anyhow::Result<()> {
    let saniye: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(30);
    let yol = concat!(env!("CARGO_MANIFEST_DIR"), "/target/mikrofon_testi.wav");

    let kayit = hound::WavWriter::create(
        yol,
        hound::WavSpec {
            channels: 1,
            sample_rate: HIZ,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )?;
    let kayit = Arc::new(Mutex::new(Some(kayit)));
    let olcum = Arc::new(Mutex::new(Olcum::default()));

    let mik = {
        let kayit = kayit.clone();
        let olcum = olcum.clone();
        Mikrofon::ac(Arc::new(AtomicBool::new(true)), move |kare, konusma| {
            if let Some(w) = kayit.lock().unwrap().as_mut() {
                for &s in kare {
                    let _ = w.write_sample(s);
                }
            }
            let mut o = olcum.lock().unwrap();
            let simdi = Instant::now();
            if let Some(onceki) = o.son_kare_zamani {
                o.en_uzun_aralik = o.en_uzun_aralik.max(simdi - onceki);
            }
            o.son_kare_zamani = Some(simdi);
            o.kare += 1;
            for &s in kare {
                o.tepe = o.tepe.max(s.abs());
                o.kare_toplam += (s * s) as f64;
            }
            o.konusma_max = o.konusma_max.max(konusma);
        })?
    };

    println!("Cihaz: {} | {} Hz | {} kanal", mik.bilgi.cihaz, mik.bilgi.cihaz_hizi, mik.bilgi.kanal);
    if mik.bilgi.cihaz_hizi != HIZ {
        println!("Not: cihaz {} Hz, 48000 Hz'e çevriliyor", mik.bilgi.cihaz_hizi);
    }
    println!("{saniye} saniye kayıt. Konuş ve bu sırada müzik ya da oyun sesi açık olsun.\n");

    let basla = Instant::now();
    let islemci_basla = islemci_suresi();
    let mut toplam_kare = 0;
    while basla.elapsed() < Duration::from_secs(saniye) {
        std::thread::sleep(Duration::from_millis(500));
        let mut o = olcum.lock().unwrap();
        let rms = if o.kare > 0 { (o.kare_toplam / (o.kare as f64 * 480.0)).sqrt() } else { 0.0 };
        let db = 20.0 * (rms.max(1e-6)).log10();
        let bar = "#".repeat(((db + 60.0).max(0.0) / 2.0) as usize);
        println!(
            "{:>5.1}s  seviye {:>6.1} dB  konuşma {:>3.0}%  {}",
            basla.elapsed().as_secs_f32(),
            db,
            o.konusma_max * 100.0,
            bar
        );
        toplam_kare += o.kare;
        let aralik = o.en_uzun_aralik;
        *o = Olcum { en_uzun_aralik: aralik, son_kare_zamani: o.son_kare_zamani, ..Default::default() };
    }
    let gecen = basla.elapsed().as_secs_f64();
    let islemci = islemci_suresi() - islemci_basla;
    drop(mik);
    kayit.lock().unwrap().take().unwrap().finalize()?;

    let o = olcum.lock().unwrap();
    println!("\nKare: {toplam_kare} (beklenen ~{:.0})", gecen * 100.0);
    println!("İki kare arası en uzun bekleme: {:.1} ms", o.en_uzun_aralik.as_secs_f64() * 1000.0);
    println!("İşlemci kullanımı: %{:.2} (tek çekirdek)", islemci / gecen * 100.0);
    println!("Kayıt: {yol}");
    Ok(())
}

/// Bu işlemin harcadığı toplam işlemci süresi (saniye).
fn islemci_suresi() -> f64 {
    let cikti = std::process::Command::new("ps")
        .args(["-o", "cputime=", "-p", &std::process::id().to_string()])
        .output();
    let Ok(c) = cikti else { return 0.0 };
    let s = String::from_utf8_lossy(&c.stdout);
    // Biçim: [[gg-]ss:]dd:ss.xx
    s.trim()
        .split(':')
        .rev()
        .enumerate()
        .map(|(i, p)| p.parse::<f64>().unwrap_or(0.0) * 60f64.powi(i as i32))
        .sum()
}

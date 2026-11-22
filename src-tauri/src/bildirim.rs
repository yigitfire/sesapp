//! Sağ üst köşede çıkan küçük bildirim penceresi (uygulama arkadayken, örneğin oyundayken).
//!
//! Oyundan çıkarmaması (alt-tab yaptırmaması) için pencere hiç gizlenip gösterilmez:
//! Windows'ta gizli bir pencereyi göstermek onu öne alıp odağı çalar. Bunun yerine pencere
//! uygulama açılırken bir kere, odak almadan ve ekranın çok dışında açılır; bildirim
//! gelince ekrana taşınır, bitince yine dışarı taşınır. Taşımak odağı değiştirmez.
//! Pencere ayrıca hiç odak alamaz (focusable false), görev çubuğunda görünmez ve hep üsttedir.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

const ETIKET: &str = "bildirim";
const GENISLIK: f64 = 340.0;
const YUKSEKLIK: f64 = 76.0;
const KENAR: f64 = 16.0;
const DISARI: i32 = -32_000;
/// Bildirim ekranda kaç ms kalır.
const SURE_MS: u64 = 5_000;
/// Fare bildirimden çekilince kaç ms sonra kapanır.
const BIRAKINCA_MS: u64 = 1_500;

/// Kapanma sayacı Rust'ta tutulur: Mac ekran dışındaki pencerenin zamanlayıcılarını yavaşlattığı
/// için sayfadaki setTimeout 5 sn yerine 15 sn sürüyordu. Her yeni bildirim/fare hareketi numarayı
/// artırır; süre dolduğunda numara değişmemişse bildirim kapanır.
static NUMARA: AtomicU64 = AtomicU64::new(0);

fn kapanmayi_kur(app: &AppHandle, ms: u64) {
    let numara = NUMARA.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(ms));
        if NUMARA.load(Ordering::SeqCst) == numara {
            bildirim_kapat(app);
        }
    });
}

pub fn olustur(app: &AppHandle) -> tauri::Result<()> {
    let pencere = WebviewWindowBuilder::new(app, ETIKET, WebviewUrl::App("bildirim.html".into()))
        .title("Monke")
        .inner_size(GENISLIK, YUKSEKLIK)
        .position(f64::from(DISARI), f64::from(DISARI))
        .decorations(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .focused(false)
        .focusable(false)
        .background_color(tauri::window::Color(17, 17, 17, 255));
    // Mac'te odak alamayan pencereye ilk tıklama da gitsin (yoksa tıklama boşa gider).
    #[cfg(target_os = "macos")]
    let pencere = pencere.accept_first_mouse(true);
    pencere.build()?;
    Ok(())
}

/// Ana sayfa çağırır: bildirimi pencereye yollar. Pencere içeriği çizince `bildirim_ac` ile
/// ekrana gelir (önce taşınırsa bir an eski bildirim görünürdü).
/// Ana pencere en öndeyse (odak bizdeyse) bildirim çıkmaz; başka pencerenin arkasındaysa
/// ya da küçültülmüşse çıkar.
#[tauri::command]
pub fn bildirim_goster(app: AppHandle, veri: serde_json::Value) {
    let ondemi = app
        .get_webview_window("main")
        .is_some_and(|ana| ana.is_focused().unwrap_or(false) && !ana.is_minimized().unwrap_or(false));
    if ondemi {
        return;
    }
    let _ = app.emit_to(ETIKET, "bildirim", veri);
}

/// Bildirim penceresini ana ekranın sağ üst köşesine taşır (görev çubuğu/menü çubuğu hariç alan).
#[tauri::command]
pub fn bildirim_ac(app: AppHandle) {
    let Some(pencere) = app.get_webview_window(ETIKET) else {
        return;
    };
    let ekran = app
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| app.get_webview_window("main").and_then(|m| m.current_monitor().ok().flatten()));
    let Some(ekran) = ekran else {
        return;
    };
    let olcek = ekran.scale_factor();
    let alan = ekran.work_area();
    let genislik = (GENISLIK * olcek).round() as i32;
    let kenar = (KENAR * olcek).round() as i32;
    let x = alan.position.x + alan.size.width as i32 - genislik - kenar;
    let y = alan.position.y + kenar;
    // Sonradan açılan "hep üstte" bir pencere (oyun) öne geçmiş olabilir: üstte olma yeniden
    // istenir (zaten açıksa tek başına bir şey yapmadığı için önce kapatılır). İkisi de odak almaz.
    let _ = pencere.set_always_on_top(false);
    let _ = pencere.set_always_on_top(true);
    let _ = pencere.set_position(PhysicalPosition::new(x, y));
    kapanmayi_kur(&app, SURE_MS);
}

/// Fare bildirimin üstüne gelince: kapanmaz.
#[tauri::command]
pub fn bildirim_bekle() {
    NUMARA.fetch_add(1, Ordering::SeqCst);
}

/// Fare bildirimden çekilince: kısa süre sonra kapanır.
#[tauri::command]
pub fn bildirim_birak(app: AppHandle) {
    kapanmayi_kur(&app, BIRAKINCA_MS);
}

/// Bildirim süresi dolunca: pencereyi yine ekran dışına taşır ve sayfaya kapandığını söyler
/// (bir sonraki bildirim yine kayarak gelsin diye).
pub fn bildirim_kapat(app: AppHandle) {
    NUMARA.fetch_add(1, Ordering::SeqCst);
    if let Some(pencere) = app.get_webview_window(ETIKET) {
        let _ = pencere.set_position(PhysicalPosition::new(DISARI, DISARI));
    }
    let _ = app.emit_to(ETIKET, "kapandi", ());
}

/// Bildirime tıklanınca: bildirim kapanır, uygulama öne gelir (kullanıcı kendisi istedi).
#[tauri::command]
pub fn bildirim_tikla(app: AppHandle) {
    bildirim_kapat(app.clone());
    if let Some(ana) = app.get_webview_window("main") {
        let _ = ana.unminimize();
        let _ = ana.show();
        let _ = ana.set_focus();
    }
}

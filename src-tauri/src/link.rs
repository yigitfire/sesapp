// Sohbetteki linkler: tarayıcıda açma ve önizleme (başlık, açıklama, resim).
//
// Önizleme sayfadan (WebView) çekilemez: siteler başka sitelerin okumasına izin vermez (CORS).
// Burada Rust çeker. Siteler önizleme bilgisini (og:title, og:image...) çoğu zaman sadece
// önizleme botlarına verir (Spotify kısa linkleri gibi); o yüzden kendini "önizleme botu"
// olarak tanıtır. Resim de burada indirilip data: URL olarak verilir: böylece sayfanın
// güvenlik ayarında (CSP) her siteye resim izni açmak gerekmez.
//
// Güvenlik: sadece http/https; yerel ağ adresleri (localhost, 192.168..., .local) çekilmez,
// yönlendirmelerde de kontrol edilir. Sayfanın en fazla 1 MB'ı, resmin en fazla 3 MB'ı okunur.

use base64::Engine;
use serde::Serialize;
use std::net::IpAddr;
use std::time::Duration;

const TARAYICI_KIMLIGI: &str = "Mozilla/5.0 (compatible; MonkeLinkBot/1.0; like Twitterbot)";
const SAYFA_SINIRI: usize = 1024 * 1024;
const RESIM_SINIRI: usize = 3 * 1024 * 1024;

#[derive(Serialize, Default)]
pub struct Onizleme {
    site: String,
    baslik: String,
    aciklama: String,
    resim: Option<String>,
    renk: Option<String>,
    buyuk: bool,
}

fn yerel_mi(url: &reqwest::Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return true;
    }
    match url.host() {
        None => true,
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost" || [".localhost", ".local", ".internal", ".lan", ".home.arpa"].iter().any(|s| d.ends_with(s))
                || !d.contains('.')
        }
        Some(url::Host::Ipv4(ip)) => ozel_ip(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => ozel_ip(IpAddr::V6(ip)),
    }
}

fn ozel_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_loopback() || v.is_link_local() || v.is_unspecified()
            || v.is_broadcast() || v.octets()[0] == 100 && (v.octets()[1] & 0xc0) == 64,
        IpAddr::V6(v) => v.is_loopback() || v.is_unspecified() || (v.segments()[0] & 0xfe00) == 0xfc00
            || (v.segments()[0] & 0xffc0) == 0xfe80
            || v.to_ipv4_mapped().is_some_and(|v4| ozel_ip(IpAddr::V4(v4))),
    }
}

fn istemci() -> Result<reqwest::Client, String> {
    // reqwest TLS için bir şifreleme sağlayıcısı ister; güncelleyici de aynısını (ring) kurar.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    reqwest::Client::builder()
        .user_agent(TARAYICI_KIMLIGI)
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(6))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() >= 6 || yerel_mi(a.url()) { a.stop() } else { a.follow() }
        }))
        .build()
        .map_err(|e| e.to_string())
}

// Cevabın en fazla `sinir` baytını okur.
async fn oku(mut cevap: reqwest::Response, sinir: usize) -> Result<Vec<u8>, String> {
    let mut veri = Vec::new();
    while let Some(parca) = cevap.chunk().await.map_err(|e| e.to_string())? {
        veri.extend_from_slice(&parca);
        if veri.len() >= sinir {
            veri.truncate(sinir);
            break;
        }
    }
    Ok(veri)
}

fn html_coz(s: &str) -> String {
    let mut sonuc = String::with_capacity(s.len());
    let mut kalan = s;
    while let Some(i) = kalan.find('&') {
        sonuc.push_str(&kalan[..i]);
        kalan = &kalan[i..];
        let son = kalan[..kalan.len().min(12)].find(';');
        let cozuldu = son.and_then(|j| {
            let ad = &kalan[1..j];
            let c = match ad {
                "amp" => Some('&'), "lt" => Some('<'), "gt" => Some('>'), "quot" => Some('"'),
                "apos" => Some('\''), "nbsp" => Some(' '),
                _ if ad.starts_with("#x") || ad.starts_with("#X") => u32::from_str_radix(&ad[2..], 16).ok().and_then(char::from_u32),
                _ if ad.starts_with('#') => ad[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            }?;
            Some((c, j))
        });
        match cozuldu {
            Some((c, j)) => { sonuc.push(c); kalan = &kalan[j + 1..]; }
            None => { sonuc.push('&'); kalan = &kalan[1..]; }
        }
    }
    sonuc.push_str(kalan);
    sonuc.split_whitespace().collect::<Vec<_>>().join(" ")
}

// Etiketteki bir özelliğin değeri (ad="değer", ad='değer' ya da ad=değer).
fn ozellik(etiket: &str, ad: &str) -> Option<String> {
    let kucuk = etiket.to_ascii_lowercase();
    let mut bas = 0;
    while let Some(i) = kucuk[bas..].find(ad) {
        let i = bas + i;
        bas = i + ad.len();
        let once = kucuk[..i].chars().last();
        if !matches!(once, Some(c) if c.is_whitespace()) { continue; }
        let sonra = kucuk[bas..].trim_start();
        if !sonra.starts_with('=') { continue; }
        let deger_bas = etiket.len() - sonra.len() + 1;
        let deger = etiket[deger_bas..].trim_start();
        return Some(match deger.chars().next() {
            Some(q @ ('"' | '\'')) => deger[1..].split(q).next().unwrap_or("").to_string(),
            _ => deger.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or("").to_string(),
        });
    }
    None
}

struct Bilgi { meta: Vec<(String, String)>, baslik: Option<String> }

fn ayikla(html: &str) -> Bilgi {
    let kucuk = html.to_ascii_lowercase();
    let mut meta = Vec::new();
    let mut bas = 0;
    while let Some(i) = kucuk[bas..].find("<meta") {
        let i = bas + i;
        let Some(j) = kucuk[i..].find('>') else { break };
        let etiket = &html[i..i + j];
        bas = i + j;
        let ad = ozellik(etiket, "property").or_else(|| ozellik(etiket, "name"));
        if let (Some(ad), Some(icerik)) = (ad, ozellik(etiket, "content")) {
            meta.push((ad.to_ascii_lowercase(), html_coz(&icerik)));
        }
    }
    let baslik = kucuk.find("<title").and_then(|i| {
        let a = i + kucuk[i..].find('>')? + 1;
        let b = a + kucuk[a..].find("</title")?;
        Some(html_coz(&html[a..b]))
    });
    Bilgi { meta, baslik }
}

fn kisalt(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n - 1).collect::<String>() + "…" }
}

#[tauri::command]
pub async fn link_onizle(url: String) -> Result<Option<Onizleme>, String> {
    let adres = reqwest::Url::parse(&url).map_err(|e| e.to_string())?;
    if yerel_mi(&adres) {
        return Ok(None);
    }
    let istemci = istemci()?;
    let cevap = istemci.get(adres).header("Accept", "text/html,application/xhtml+xml").send().await
        .map_err(|e| e.to_string())?;
    if !cevap.status().is_success() {
        return Ok(None);
    }
    let son_adres = cevap.url().clone();
    let tur = cevap.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    // Link doğrudan bir resimse resmin kendisi gösterilir.
    if tur.starts_with("image/") && !tur.contains("svg") {
        let veri = oku(cevap, RESIM_SINIRI + 1).await?;
        if veri.len() > RESIM_SINIRI { return Ok(None); }
        return Ok(Some(Onizleme {
            resim: Some(format!("data:{};base64,{}", tur.split(';').next().unwrap_or("image/png"),
                base64::engine::general_purpose::STANDARD.encode(veri))),
            buyuk: true,
            ..Default::default()
        }));
    }
    if !tur.contains("html") {
        return Ok(None);
    }
    let html = String::from_utf8_lossy(&oku(cevap, SAYFA_SINIRI).await?).into_owned();
    let b = ayikla(&html);
    let al = |adlar: &[&str]| adlar.iter().find_map(|a| b.meta.iter().find(|(k, v)| k == a && !v.is_empty()).map(|(_, v)| v.clone()));
    let baslik = al(&["og:title", "twitter:title"]).or(b.baslik.clone()).unwrap_or_default();
    let aciklama = al(&["og:description", "twitter:description", "description"]).unwrap_or_default();
    let site = al(&["og:site_name"]).unwrap_or_else(|| son_adres.host_str().unwrap_or("").trim_start_matches("www.").to_string());
    let renk = al(&["theme-color"]).filter(|r| r.len() <= 9 && r.starts_with('#') && r[1..].chars().all(|c| c.is_ascii_hexdigit()));
    let buyuk = al(&["twitter:card"]).is_some_and(|c| c == "summary_large_image" || c == "player");
    if baslik.is_empty() && aciklama.is_empty() {
        return Ok(None);
    }
    let mut resim = None;
    if let Some(r) = al(&["og:image", "og:image:url", "og:image:secure_url", "twitter:image", "twitter:image:src"])
        .and_then(|r| son_adres.join(&r).ok())
        .filter(|r| !yerel_mi(r))
    {
        if let Ok(c) = istemci.get(r).header("Accept", "image/*").send().await {
            let tur = c.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
            if c.status().is_success() && tur.starts_with("image/") && !tur.contains("svg") {
                if let Ok(veri) = oku(c, RESIM_SINIRI + 1).await {
                    if veri.len() <= RESIM_SINIRI {
                        resim = Some(format!("data:{};base64,{}", tur.split(';').next().unwrap_or("image/jpeg"),
                            base64::engine::general_purpose::STANDARD.encode(veri)));
                    }
                }
            }
        }
    }
    Ok(Some(Onizleme {
        site: kisalt(&site, 60),
        baslik: kisalt(&baslik, 200),
        aciklama: kisalt(&aciklama, 350),
        resim,
        renk,
        buyuk,
    }))
}

// Linki varsayılan tarayıcıda açar. Kabuk (shell) kullanılmaz; adres tek argüman olarak verilir.
#[tauri::command]
pub fn link_ac(url: String) -> Result<(), String> {
    let adres = reqwest::Url::parse(&url).map_err(|e| e.to_string())?;
    if !matches!(adres.scheme(), "http" | "https") {
        return Err("sadece http/https".into());
    }
    #[cfg(target_os = "macos")]
    let sonuc = std::process::Command::new("open").arg(adres.as_str()).spawn();
    #[cfg(target_os = "windows")]
    let sonuc = {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("rundll32").arg("url.dll,FileProtocolHandler").arg(adres.as_str())
            .creation_flags(0x0800_0000).spawn()
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let sonuc = std::process::Command::new("xdg-open").arg(adres.as_str()).spawn();
    sonuc.map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod testler {
    use super::*;

    #[test]
    fn yerel_adresler_cekilmez() {
        for u in ["http://localhost:8080/", "http://192.168.1.1/", "http://10.0.0.5/a", "http://127.0.0.1/", "http://[::1]/",
            "http://yazici.local/", "http://router/", "file:///etc/passwd", "http://169.254.169.254/latest/"] {
            assert!(yerel_mi(&reqwest::Url::parse(u).unwrap()), "{u}");
        }
        assert!(!yerel_mi(&reqwest::Url::parse("https://open.spotify.com/x").unwrap()));
    }

    #[test]
    fn html_cozulur() {
        assert_eq!(html_coz("Spotify&#x27;da Jam &amp; &quot;x&quot; &#304;"), "Spotify'da Jam & \"x\" İ");
        let b = ayikla(r#"<head><title>T</title><meta property="og:title" content="Başlık &amp; b"/><meta content='Açıklama' name=description></head>"#);
        assert_eq!(b.baslik.as_deref(), Some("T"));
        assert!(b.meta.contains(&("og:title".into(), "Başlık & b".into())));
        assert!(b.meta.contains(&("description".into(), "Açıklama".into())));
    }

    // İnternet ister: cargo test link -- --ignored --nocapture
    #[test]
    #[ignore]
    fn gercek_siteler() {
        for u in ["https://spotify.link/NzBRfIzXk8b", "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT", "https://github.com/yigitfire/sesapp",
            "https://yigitfire.github.io/sesapp/logo.svg", "https://upload.wikimedia.org/wikipedia/commons/thumb/a/a7/Camponotus_flavomarginatus_ant.jpg/320px-Camponotus_flavomarginatus_ant.jpg"] {
            let o = tauri::async_runtime::block_on(link_onizle(u.into()));
            match o {
                Ok(Some(o)) => println!("{u}\n  site={} baslik={} buyuk={} renk={:?} resim={}\n  aciklama={}", o.site, o.baslik, o.buyuk, o.renk,
                    o.resim.as_ref().map(|r| format!("{} bayt ({})", r.len(), &r[..r.find(';').unwrap_or(0)])).unwrap_or("yok".into()), o.aciklama.chars().take(80).collect::<String>()),
                other => println!("{u}\n  -> {:?}", other.map(|x| x.is_some())),
            }
        }
    }
}

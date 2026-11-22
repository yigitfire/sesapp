// Windows'ta uygulama açılınca arkada siyah konsol penceresi çıkmasın.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    sesapp::calistir();
}

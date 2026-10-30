//! Text insertion (plan §6.4 §5): one clipboard paste, then restore the user's clipboard.

use enigo::{Direction::{Click, Press, Release}, Enigo, Key, Keyboard, Settings};
use std::{thread::sleep, time::Duration};

#[cfg(target_os = "macos")]
const MOD: Key = Key::Meta;
#[cfg(not(target_os = "macos"))]
const MOD: Key = Key::Control;

#[cfg(windows)]
fn held(vk: i32) -> bool {
    #[link(name = "user32")]
    extern "system" { fn GetAsyncKeyState(vk: i32) -> i16; }
    unsafe { GetAsyncKeyState(vk) < 0 }
}

fn enigo() -> Result<Enigo, String> {
    let mut e = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    // The push-to-talk chord may still be held; release modifiers so Ctrl+V isn't Ctrl+Shift+V.
    // Windows: only keys that are actually down. A lone Alt key-up makes Office show its ribbon
    // KeyTips and swallow the paste, so a held Alt is masked with an unassigned key (VK 0xE8) first.
    #[cfg(windows)]
    for (k, vk) in [(Key::Shift, 0x10), (Key::Alt, 0x12), (Key::Control, 0x11), (Key::LWin, 0x5B), (Key::RWin, 0x5C)] {
        if !held(vk) { continue; }
        if vk == 0x12 { let _ = e.key(Key::Other(0xE8), Click); }
        let _ = e.key(k, Release);
    }
    #[cfg(not(windows))]
    for k in [Key::Shift, Key::Alt, Key::Control, Key::Meta] {
        let _ = e.key(k, Release);
    }
    Ok(e)
}

fn chord(e: &mut Enigo, c: char) -> Result<(), String> {
    e.key(MOD, Press).map_err(|x| x.to_string())?;
    let r = e.key(Key::Unicode(c), Click).map_err(|x| x.to_string());
    let _ = e.key(MOD, Release);
    r
}

// ponytail: only text clipboard contents are restored; images/files on the clipboard are lost.
pub fn paste(text: &str) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    let prev = cb.get_text().ok();
    cb.set_text(text.to_string()).map_err(|e| e.to_string())?;
    sleep(Duration::from_millis(30));
    let mut e = enigo()?;
    chord(&mut e, 'v')?;
    sleep(Duration::from_millis(150));
    if let Some(p) = prev {
        let _ = cb.set_text(p);
    }
    Ok(())
}

pub fn undo() -> Result<(), String> {
    chord(&mut enigo()?, 'z')
}

pub fn select_all() -> Result<(), String> {
    chord(&mut enigo()?, 'a')
}

pub fn enter() -> Result<(), String> {
    enigo()?.key(Key::Return, Click).map_err(|e| e.to_string())
}

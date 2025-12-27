extern crate evdev_rs as evdev;
extern crate mio;

use evdev::*;
use evdev::enums::*;
use std::io;
use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::os::unix::io::AsRawFd;
use mio::{Poll, Events, Token, Interest};
use mio::unix::SourceFd;
use std::time::Duration;

// ---------------- 配置支持 ----------------
use std::collections::HashMap;

// ---------------- 连发线程依赖 ----------------
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicBool, Ordering},
};
use std::thread;

// ================== Repeat Action ==================
#[derive(Clone, Copy, PartialEq)]
enum RepeatAction {
    None = 0,
    BrightUp,
    BrightDown,
    VolUp,
    VolDown,
}

// ================== Key Mapping ==================
struct Keys {
    hotkey:         EventCode,
    bright_up:      EventCode,
    bright_down:    EventCode,
    vol_up:         EventCode,
    vol_down:       EventCode,
    volume_up:      EventCode,
    volume_down:    EventCode,
    mute:           EventCode,
}

fn default_keys() -> Keys {
    Keys {
        hotkey:       EventCode::EV_KEY(EV_KEY::BTN_TRIGGER_HAPPY5),
        bright_up:    EventCode::EV_KEY(EV_KEY::BTN_DPAD_UP),
        bright_down:  EventCode::EV_KEY(EV_KEY::BTN_DPAD_DOWN),
        vol_up:       EventCode::EV_KEY(EV_KEY::BTN_DPAD_RIGHT),
        vol_down:     EventCode::EV_KEY(EV_KEY::BTN_DPAD_LEFT),
        volume_up:    EventCode::EV_KEY(EV_KEY::KEY_VOLUMEUP),
        volume_down:  EventCode::EV_KEY(EV_KEY::KEY_VOLUMEDOWN),
        mute:         EventCode::EV_KEY(EV_KEY::KEY_PLAYPAUSE),
    }
}

fn parse_ev_key(name: &str) -> Option<EventCode> {
    use EV_KEY::*;
    let key = match name {
        "BTN_DPAD_UP"    => BTN_DPAD_UP,
        "BTN_DPAD_DOWN"  => BTN_DPAD_DOWN,
        "BTN_DPAD_LEFT"  => BTN_DPAD_LEFT,
        "BTN_DPAD_RIGHT" => BTN_DPAD_RIGHT,

        "BTN_TRIGGER_HAPPY1" => BTN_TRIGGER_HAPPY1,
        "BTN_TRIGGER_HAPPY4" => BTN_TRIGGER_HAPPY4,
        "BTN_TRIGGER_HAPPY5" => BTN_TRIGGER_HAPPY5,

        "KEY_VOLUMEUP"   => KEY_VOLUMEUP,
        "KEY_VOLUMEDOWN" => KEY_VOLUMEDOWN,
        "KEY_PLAYPAUSE"  => KEY_PLAYPAUSE,
        "KEY_POWER"      => KEY_POWER,
        _ => return None,
    };
    Some(EventCode::EV_KEY(key))
}

fn load_keys_from_conf(path: &str) -> Keys {
    let mut keys = default_keys();
    let data = std::fs::read_to_string(path).unwrap_or_default();
    let mut map: HashMap<String, String> = HashMap::new();

    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }

    let set = |field: &str, dst: &mut EventCode| {
        if let Some(name) = map.get(field) {
            if let Some(code) = parse_ev_key(name) {
                *dst = code;
            }
        }
    };

    set("HOTKEY",        &mut keys.hotkey);
    set("BRIGHT_UP",     &mut keys.bright_up);
    set("BRIGHT_DOWN",   &mut keys.bright_down);
    set("VOL_UP",        &mut keys.vol_up);
    set("VOL_DOWN",      &mut keys.vol_down);
    set("VOLUME_UP",     &mut keys.volume_up);
    set("VOLUME_DOWN",   &mut keys.volume_down);
    set("MUTE",          &mut keys.mute);

    keys
}

// ================== Event Processing ==================
fn process_event(
    ev: &InputEvent,
    hotkey: bool,
    k: &Keys,
    repeat_action: &Arc<AtomicU8>,
    repeat_active: &Arc<AtomicBool>,
) {
    let pressed = ev.value == 1 || ev.value == 2;

    // -------- 单独音量键：使用内核 autorepeat --------
    if ev.event_code == k.volume_up && pressed {
        let _ = Command::new("amixer")
            .args(&["-q", "sset", "Playback", "1%+"])
            .spawn();
        return;
    } else if ev.event_code == k.volume_down && pressed {
        let _ = Command::new("amixer")
            .args(&["-q", "sset", "Playback", "1%-"])
            .spawn();
        return;
    }

    // -------- hotkey + DPAD：线程连发 --------
    if hotkey && ev.value == 1 {
        if ev.event_code == k.bright_up {
            repeat_action.store(RepeatAction::BrightUp as u8, Ordering::Relaxed);
            repeat_active.store(true, Ordering::Relaxed);
        } else if ev.event_code == k.bright_down {
            repeat_action.store(RepeatAction::BrightDown as u8, Ordering::Relaxed);
            repeat_active.store(true, Ordering::Relaxed);
        } else if ev.event_code == k.vol_up {
            repeat_action.store(RepeatAction::VolUp as u8, Ordering::Relaxed);
            repeat_active.store(true, Ordering::Relaxed);
        } else if ev.event_code == k.vol_down {
            repeat_action.store(RepeatAction::VolDown as u8, Ordering::Relaxed);
            repeat_active.store(true, Ordering::Relaxed);
        }
    }

    // -------- 松手：停止线程连发 --------
    if ev.value == 0 {
        if ev.event_code == k.bright_up
            || ev.event_code == k.bright_down
            || ev.event_code == k.vol_up
            || ev.event_code == k.vol_down
        {
            repeat_action.store(RepeatAction::None as u8, Ordering::Relaxed);
            repeat_active.store(false, Ordering::Relaxed);
        }
    }

    if ev.event_code == k.mute && ev.value == 1 {
        let _ = Command::new("mute_toggle.sh").spawn();
    }
}

fn process_event2(ev: &InputEvent, selectkey: bool) {
    if selectkey && ev.event_code == EventCode::EV_KEY(EV_KEY::BTN_TRIGGER_HAPPY4) && ev.value == 1 {
        let _ = Command::new("speak_bat_life.sh").spawn();
    }
}

// ================== Main ==================
fn main() -> io::Result<()> {
    let keys = load_keys_from_conf("/home/ark/ogage.conf");

    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(1);
    let mut devs: Vec<Device> = Vec::new();

    let mut hotkey = false;
    let mut selectkey = false;

    let repeat_action = Arc::new(AtomicU8::new(RepeatAction::None as u8));
    let repeat_active = Arc::new(AtomicBool::new(false));

    // -------- 连发后台线程 --------
    {
        let repeat_action = repeat_action.clone();
        let repeat_active = repeat_active.clone();

        thread::spawn(move || {
            loop {
                if repeat_active.load(Ordering::Relaxed) {
                    match unsafe {
                        std::mem::transmute::<u8, RepeatAction>(
                            repeat_action.load(Ordering::Relaxed),
                        )
                    } {
                        RepeatAction::BrightUp => {
                            let _ = Command::new("brightnessctl").args(&["s", "+2%"]).output();
                        }
                        RepeatAction::BrightDown => {
                            let _ = Command::new("brightnessctl").args(&["-n", "s", "2%-"]).output();
                        }
                        RepeatAction::VolUp => {
                            let _ = Command::new("amixer").args(&["-q", "sset", "Playback", "1%+"]).output();
                        }
                        RepeatAction::VolDown => {
                            let _ = Command::new("amixer").args(&["-q", "sset", "Playback", "1%-"]).output();
                        }
                        RepeatAction::None => {}
                    }
                }
                thread::sleep(Duration::from_millis(120));
            }
        });
    }

    // -------- 打开 input 设备 --------
    let mut i = 0;
    for s in [
        "/dev/input/event10","/dev/input/event9","/dev/input/event8",
        "/dev/input/event7","/dev/input/event6","/dev/input/event5",
        "/dev/input/event4","/dev/input/event3","/dev/input/event2",
        "/dev/input/event1","/dev/input/event0"
    ] {
        if !Path::new(s).exists() {
            continue;
        }

        let file = File::open(s)?;
        poll.registry().register(
            &mut SourceFd(&file.as_raw_fd()),
            Token(i),
            Interest::READABLE,
        )?;

        let dev = Device::new_from_file(file)?;
        devs.push(dev);
        i += 1;
    }

    // -------- 主事件循环 --------
    loop {
        poll.poll(&mut events, None)?;
        for event in events.iter() {
            let dev = &mut devs[event.token().0];
            while dev.has_event_pending() {
                if let Ok(kv) = dev.next_event(evdev_rs::ReadFlag::NORMAL) {
                    let ev = &kv.1;

                    if ev.event_code == keys.hotkey {
                        hotkey = ev.value == 1 || ev.value == 2;
                        if !hotkey {
                            repeat_action.store(RepeatAction::None as u8, Ordering::Relaxed);
                            repeat_active.store(false, Ordering::Relaxed);
                        }
                    }

                    if ev.event_code == EventCode::EV_KEY(EV_KEY::BTN_TRIGGER_HAPPY1) {
                        selectkey = ev.value == 1 || ev.value == 2;
                    }

                    process_event(ev, hotkey, &keys, &repeat_action, &repeat_active);
                    process_event2(ev, selectkey);
                }
            }
        }
    }
}

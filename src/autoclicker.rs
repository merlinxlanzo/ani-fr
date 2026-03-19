use device_query::{DeviceQuery, DeviceState, Keycode};
use mouse_rs::{types::keys::Keys, Mouse};
use rand::RngExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[cfg(target_os = "windows")]
mod hook {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    static mut MOUSE_PHYSICALLY_HELD: bool = false;
    static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

    const WH_MOUSE_LL: i32 = 14;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_LBUTTONUP: u32 = 0x0202;
    const HC_ACTION: i32 = 0;

    type HOOKPROC = unsafe extern "system" fn(i32, usize, isize) -> isize;

    extern "system" {
        fn SetWindowsHookExW(id_hook: i32, lpfn: HOOKPROC, hmod: isize, thread_id: u32) -> isize;
        fn CallNextHookEx(hhk: isize, n_code: i32, w_param: usize, l_param: isize) -> isize;
        fn GetMessageW(msg: *mut u8, hwnd: isize, filter_min: u32, filter_max: u32) -> i32;
    }

    #[repr(C)]
    struct MSLLHOOKSTRUCT {
        pt_x: i32,
        pt_y: i32,
        mouse_data: u32,
        flags: u32,
        time: u32,
        dw_extra_info: usize,
    }

    unsafe extern "system" fn mouse_hook_proc(n_code: i32, w_param: usize, l_param: isize) -> isize {
        if n_code == HC_ACTION {
            let info = &*(l_param as *const MSLLHOOKSTRUCT);
            if info.flags & 1 == 0 {
                match w_param as u32 {
                    WM_LBUTTONDOWN => { MOUSE_PHYSICALLY_HELD = true; }
                    WM_LBUTTONUP => { MOUSE_PHYSICALLY_HELD = false; }
                    _ => {}
                }
            }
        }
        CallNextHookEx(0, n_code, w_param, l_param)
    }

    pub fn install_hook() {
        if HOOK_INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        thread::spawn(|| {
            unsafe {
                SetWindowsHookExW(WH_MOUSE_LL, mouse_hook_proc, 0, 0);
                let mut msg = [0u8; 48];
                loop {
                    GetMessageW(msg.as_mut_ptr(), 0, 0, 0);
                }
            }
        });
        thread::sleep(Duration::from_millis(50));
    }

    pub fn is_held() -> bool {
        unsafe { MOUSE_PHYSICALLY_HELD }
    }
}

struct EpisodeTimer {
    base_interval_ms: f64,
    tick: u64,
    prev_intervals: [f64; 8],
    trend: f64,
}

impl EpisodeTimer {
    fn new(episode: u32) -> Self {
        let base = 1000.0 / episode as f64;
        Self {
            base_interval_ms: base,
            tick: 0,
            prev_intervals: [base; 8],
            trend: 0.0,
        }
    }

    fn next_frame_ms(&mut self) -> f64 {
        let mut rng = rand::rng();
        self.tick += 1;

        let gaussian: f64 = (0..12)
            .map(|_| rng.random_range(0.0f64..1.0f64))
            .sum::<f64>()
            - 6.0;
        let jitter_pct = gaussian * 0.12;

        let drift_slow = (self.tick as f64 * 0.005).sin() * 0.06;
        let drift_fast = (self.tick as f64 * 0.037).sin() * 0.03;
        let drift = drift_slow + drift_fast;

        let avg_recent: f64 =
            self.prev_intervals.iter().sum::<f64>() / self.prev_intervals.len() as f64;
        let momentum = ((avg_recent / self.base_interval_ms) - 1.0) * 0.3;

        if rng.random_range(0u32..100) < 3 {
            self.trend = rng.random_range(-0.05f64..0.05f64);
        }

        let burst = if rng.random_range(0u32..100) < 4 {
            rng.random_range(0.45f64..0.65f64)
        } else {
            1.0
        };

        let hesitation = if rng.random_range(0u32..100) < 3 {
            1.0 + rng.random_range(0.15f64..0.45f64)
        } else {
            1.0
        };

        let group_size = 3 + (self.tick / 50 % 3);
        let rhythm = if self.tick % group_size == 0 {
            1.0 + rng.random_range(0.03f64..0.09f64)
        } else {
            1.0
        };

        let micro_noise = rng.random_range(-0.02f64..0.02f64);

        let interval = self.base_interval_ms
            * (1.0 + jitter_pct + drift + momentum + self.trend + micro_noise)
            * burst
            * hesitation
            * rhythm;

        let clamped = interval.clamp(self.base_interval_ms * 0.4, self.base_interval_ms * 2.5);
        self.prev_intervals[self.tick as usize % self.prev_intervals.len()] = clamped;
        clamped
    }
}

pub fn run_episode(episode: u32) {
    println!("F7 pour lancer/mettre en pause la lecture, maintenir clic gauche pour avancer, F8 pour arrêter l'épisode.");

    hook::install_hook();

    let device_state = DeviceState::new();
    let enabled = Arc::new(AtomicBool::new(false));
    let mut f7_was_pressed = false;
    let mut f8_was_pressed = false;

    loop {
        let keys = device_state.get_keys();

        let f7_pressed = keys.contains(&Keycode::F7);
        if f7_pressed && !f7_was_pressed {
            if !enabled.load(Ordering::SeqCst) {
                enabled.store(true, Ordering::SeqCst);
                println!("  ▶ Lecture en cours");

                let click_enabled = Arc::clone(&enabled);
                let ep = episode;
                thread::spawn(move || {
                    let mouse = Mouse::new();
                    let mut timer = EpisodeTimer::new(ep);

                    while click_enabled.load(Ordering::SeqCst) {
                        if !hook::is_held() {
                            thread::sleep(Duration::from_millis(5));
                            continue;
                        }

                        while hook::is_held() && click_enabled.load(Ordering::SeqCst) {
                            let hold_ms = rand::rng().random_range(10.0f64..18.0);

                            let _ = mouse.press(&Keys::LEFT);
                            thread::sleep(Duration::from_micros((hold_ms * 1000.0) as u64));
                            let _ = mouse.release(&Keys::LEFT);

                            let total_delay = timer.next_frame_ms();
                            let remaining = (total_delay - hold_ms).max(1.0);
                            thread::sleep(Duration::from_micros((remaining * 1000.0) as u64));
                        }
                    }
                });
            } else {
                enabled.store(false, Ordering::SeqCst);
                println!("  ⏸ En pause");
            }
        }
        f7_was_pressed = f7_pressed;

        let f8_pressed = keys.contains(&Keycode::F8);
        if f8_pressed && !f8_was_pressed {
            enabled.store(false, Ordering::SeqCst);
            println!("\n  ■ Épisode terminé.");
            break;
        }
        f8_was_pressed = f8_pressed;

        thread::sleep(Duration::from_millis(10));
    }
}

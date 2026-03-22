use device_query::{DeviceQuery, DeviceState, Keycode};
use mouse_rs::{types::keys::Keys, Mouse};
use rand::RngExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[cfg(target_os = "windows")]
mod inner {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    static mut STATE: bool = false;
    static READY: AtomicBool = AtomicBool::new(false);

    const H: i32 = 14;
    const D: u32 = 0x0201;
    const U: u32 = 0x0202;
    const A: i32 = 0;

    type P = unsafe extern "system" fn(i32, usize, isize) -> isize;

    extern "system" {
        fn SetWindowsHookExW(a: i32, b: P, c: isize, d: u32) -> isize;
        fn CallNextHookEx(a: isize, b: i32, c: usize, d: isize) -> isize;
        fn GetMessageW(a: *mut u8, b: isize, c: u32, d: u32) -> i32;
    }

    #[repr(C)]
    struct S {
        _a: i32,
        _b: i32,
        _c: u32,
        f: u32,
        _d: u32,
        _e: usize,
    }

    unsafe extern "system" fn cb(n: i32, w: usize, l: isize) -> isize {
        if n == A {
            let s = &*(l as *const S);
            if s.f & 1 == 0 {
                match w as u32 {
                    D => { STATE = true; }
                    U => { STATE = false; }
                    _ => {}
                }
            }
        }
        CallNextHookEx(0, n, w, l)
    }

    pub fn init() {
        if READY.swap(true, Ordering::SeqCst) {
            return;
        }
        thread::spawn(|| {
            unsafe {
                SetWindowsHookExW(H, cb, 0, 0);
                let mut m = [0u8; 48];
                loop {
                    GetMessageW(m.as_mut_ptr(), 0, 0, 0);
                }
            }
        });
        thread::sleep(Duration::from_millis(50));
    }

    pub fn active() -> bool {
        unsafe { STATE }
    }
}

struct Seq {
    base: f64,
    n: u64,
    buf: [f64; 8],
    d: f64,
}

impl Seq {
    fn new(v: u32) -> Self {
        let b = 1000.0 / v as f64;
        Self { base: b, n: 0, buf: [b; 8], d: 0.0 }
    }

    fn next(&mut self) -> f64 {
        let mut r = rand::rng();
        self.n += 1;

        let g: f64 = (0..12).map(|_| r.random_range(0.0f64..1.0f64)).sum::<f64>() - 6.0;
        let j = g * 0.12;

        let s = (self.n as f64 * 0.005).sin() * 0.06;
        let f = (self.n as f64 * 0.037).sin() * 0.03;

        let a: f64 = self.buf.iter().sum::<f64>() / self.buf.len() as f64;
        let m = ((a / self.base) - 1.0) * 0.3;

        if r.random_range(0u32..100) < 3 {
            self.d = r.random_range(-0.05f64..0.05f64);
        }

        let b = if r.random_range(0u32..100) < 4 {
            r.random_range(0.45f64..0.65f64)
        } else { 1.0 };

        let h = if r.random_range(0u32..100) < 3 {
            1.0 + r.random_range(0.15f64..0.45f64)
        } else { 1.0 };

        let gs = 3 + (self.n / 50 % 3);
        let rh = if self.n % gs == 0 {
            1.0 + r.random_range(0.03f64..0.09f64)
        } else { 1.0 };

        let mn = r.random_range(-0.02f64..0.02f64);

        let v = self.base * (1.0 + j + s + f + m + self.d + mn) * b * h * rh;
        let c = v.clamp(self.base * 0.4, self.base * 2.5);
        self.buf[self.n as usize % self.buf.len()] = c;
        c
    }
}

pub fn run_episode(ep: u32) {
    println!("F7 pour lancer/mettre en pause la lecture, maintenir clic gauche pour avancer, F8 pour arr\u{ea}ter l'\u{e9}pisode.");

    inner::init();

    let ds = DeviceState::new();
    let on = Arc::new(AtomicBool::new(false));
    let mut p7 = false;
    let mut p8 = false;

    loop {
        let k = ds.get_keys();

        let f7 = k.contains(&Keycode::F7);
        if f7 && !p7 {
            if !on.load(Ordering::SeqCst) {
                on.store(true, Ordering::SeqCst);
                println!("  \u{25b6} Lecture en cours");

                let flag = Arc::clone(&on);
                thread::spawn(move || {
                    let dev = Mouse::new();
                    let mut seq = Seq::new(ep);

                    while flag.load(Ordering::SeqCst) {
                        if !inner::active() {
                            thread::sleep(Duration::from_millis(5));
                            continue;
                        }

                        while inner::active() && flag.load(Ordering::SeqCst) {
                            let t = rand::rng().random_range(10.0f64..18.0);
                            let _ = dev.press(&Keys::LEFT);
                            thread::sleep(Duration::from_micros((t * 1000.0) as u64));
                            let _ = dev.release(&Keys::LEFT);
                            let w = (seq.next() - t).max(1.0);
                            thread::sleep(Duration::from_micros((w * 1000.0) as u64));
                        }
                    }
                });
            } else {
                on.store(false, Ordering::SeqCst);
                println!("  \u{23f8} En pause");
            }
        }
        p7 = f7;

        let f8 = k.contains(&Keycode::F8);
        if f8 && !p8 {
            on.store(false, Ordering::SeqCst);
            println!("\n  \u{25a0} \u{c9}pisode termin\u{e9}.");
            break;
        }
        p8 = f8;

        thread::sleep(Duration::from_millis(10));
    }
}

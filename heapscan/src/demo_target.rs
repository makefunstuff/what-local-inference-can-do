//! Dummy process used by `heapscan demo`.
//!
//! Allocates a handful of interesting heap objects and prints them every tick,
//! so that heap writes made by the scanner are observable:
//!   * a heap `String` ("Hello, heap!")
//!   * a heap `Vec<u8>` (raw data)
//!   * a `Box<dyn Greet>` — a heap object with a vtable pointer in a code region
//!
//! Cross-platform (macOS + Linux), std only.

use std::io::Write;
use std::time::Duration;

trait Greet {
    #[allow(dead_code)]
    fn greeting(&self) -> &'static str;
}
struct Penguin {
    label: &'static str,
}

impl Greet for Penguin {
    fn greeting(&self) -> &'static str {
        self.label
    }
}

fn main() {
    let greeting = "Hello, heap!".to_string();
    let data: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04];
    let obj: Box<dyn Greet> = Box::new(Penguin {
        label: "penguin says squawk",
    });

    let mut tick: u64 = 0;
    loop {
        tick += 1;
        println!(
            "tick={} greeting={} data={} obj={:p}",
            tick,
            greeting,
            data.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
            obj,
        );
        std::io::stdout().flush().ok();
        std::thread::sleep(Duration::from_millis(500));
    }
}

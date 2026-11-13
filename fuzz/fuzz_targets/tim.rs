#![no_main]
//! TIM image parsing and decoding with every CLUT row.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(t) = ok_br1_converter::tim::Tim::parse(data) {
        if (t.width as u64) * (t.height as u64) > 1 << 22 {
            return;
        }
        for c in 0..t.cluts.len().clamp(1, 4) {
            let _ = t.image(c);
        }
    }
});

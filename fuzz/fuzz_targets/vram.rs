#![no_main]
//! TIM upload into emulated VRAM and texture-page decoding.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut v = ok_br1_converter::vram::Vram::new();
    if v.load_tim(data) && data.len() >= 4 {
        let tsb = u16::from_le_bytes([data[0], data[1]]);
        let cba = u16::from_le_bytes([data[2], data[3]]);
        let _ = v.page(tsb, cba);
    }
});

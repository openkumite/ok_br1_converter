#![no_main]
//! Sony TMD model parsing (objects, vertices, all primitive layouts).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = ok_br1_converter::tmd::parse(data);
});

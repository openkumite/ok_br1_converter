#![no_main]
//! Whole .DAT container: offset table + every compressed entry.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = ok_br1_converter::pack::unpack(data);
});

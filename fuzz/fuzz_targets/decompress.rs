#![no_main]
//! LZSS-style container entry decompressor (pack::decompress).
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = ok_br1_converter::pack::decompress(data);
});

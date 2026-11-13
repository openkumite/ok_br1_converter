#![no_main]
//! VAB sound bank parsing and VAG ADPCM decoding.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = ok_br1_converter::audio::parse_vab(data);
    let _ = ok_br1_converter::audio::decode_vag(data);
});

#![no_main]
//! XX.ANM skeleton/animation parsing plus posing every animation's first frame.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(anm) = ok_br1_converter::anim::Anm::parse(data) {
        for form in 0..anm.forms.len() {
            for a in anm.anims.iter().take(8) {
                let p = anm.pose(form, a, 0.0);
                let _ = ok_br1_converter::anim::world(&anm.forms[form].parents, &p);
            }
        }
    }
});

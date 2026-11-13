# ok_br1_converter

Converts your own Bloody Roar disc (PlayStation, USA, SCUS-94199) into modern files: the fighters as skinned glTF
models with every animation and costume, the stages as glTF scenes, textures and pictures as PNG, the music as FLAC,
the sound banks as WAV, the movies as MP4 and the particle effects, all browsable in an offline HTML gallery. As a
library it also converts the disc into runtime data (the game's raw files, the static tables cut out of its
executable, models, stages, music and movies), step by step, in native and WebAssembly programs.

> **Unofficial.** ok_br1_converter is not affiliated with, endorsed or sponsored by Konami or Sony Interactive
> Entertainment. Bloody Roar is a trademark of Konami Digital Entertainment (the series was developed by Hudson Soft,
> now part of Konami); PlayStation is a trademark of Sony Interactive Entertainment Inc. This repository contains no
> game data: you need your own legally obtained copy of the disc, which the converter reads on your computer. Do not
> ask for, or share, disc images or converted files.

## Quick start

You need [Rust](https://rustup.rs), [ffmpeg](https://ffmpeg.org) on your PATH (FLAC / MP4 encoding) and the disc as
BIN/CUE (`Bloody Roar (USA).cue` with its `(Track 1).bin` and `(Track 2).bin`).

```sh
cargo build --release
target/release/okconv-br1 convert "path/to/Bloody Roar (USA)" -o out    # the folder holding the .cue
open out/index.html                                                     # the gallery; works from file://
```

## Commands

| command | what |
|---|---|
| `okconv-br1 convert DISC [-o OUT] [--only STEPS]` | converts everything and builds the gallery; `--only images,music,sfx,video,textures,models` runs selected steps (the manifest is merged) |
| `okconv-br1 list DISC` | lists the files on the disc |
| `okconv-br1 extract DISC [-o OUT]` | copies every disc file verbatim into `OUT/raw` |
| `okconv-br1 unpack DISC [-o OUT]` | decompresses every `.DAT` container entry into `OUT/unpacked` |
| `okconv-br1 fighter DISC ID COSTUME,... [-o OUT]` | exports one fighter with any costume containers, e.g. `ZIN URI/URI10.DAT,URI/URI11.DAT` (to check a texture / mesh pairing) |

## Output (`convert`)

| from the disc | output |
|---|---|
| fighters' meshes, skeletons, animations, state scripts and costumes | `models/XX_human.glb`, `XX_beast.glb`: skinned glTF, every costume a `KHR_materials_variants` variant, the form's clips named from the move data, with part swaps, root motion and sound cues in `extras` |
| stage scenes | `models/*_stage.glb` (world space, with sky band and floor) and `textures/Backgrounds/*_panorama.png` |
| the effect sheet and particle tables | `vfx/particles/*.png`, `vfx/sparks/*.png` (animated sprite sheets, 60 ticks/s), `vfx/vfx.json`, `models/EFFECT.glb` |
| textures in the compressed containers | `textures/<container>/*.png` |
| pictures (`ART*/*.TIM`) | `images/**/*.png` |
| XA music and the CD-DA track | `music/*.flac` |
| sound banks (VAB) | `sfx/<bank>/*.wav` |
| movies (STR) | `video/*.mp4` with a poster `.jpg` |
| a PS1 look | `shaders/` (Godot `.gdshader`, GLSL); the gallery has a PS1 toggle |

All exported `.glb` files pass the Khronos glTF Validator without errors or warnings.

## Library and runtime data

Features: `cli` (default: the command line tool, the gallery, folder output, ffmpeg) and `runtime`. Without `cli`
the library uses no file system, runs no external programs and has no `anyhow` (errors are
`ok_br1_converter::Error`); it builds for `wasm32-unknown-unknown`
(`cargo build --no-default-features --features runtime --target wasm32-unknown-unknown`).

`runtime::Converter` converts the disc into runtime data: the disc comes in as a `disc::Sectors` (the data track's
sectors, raw 2352 or ISO 2048 bytes, plus CD-DA tracks; the caller opens the image), the files go out through a
`runtime::Sink`; `step` runs one step (progress in between), `run` all of them.

```rust
let disc = ok_br1_converter::disc::Disc::open_checked(sectors)?;   // Error::WrongDisc for another disc
let summary = ok_br1_converter::runtime::Converter::new(disc).run(&mut sink, &mut |p| {
    println!("{}/{} {}", p.done, p.total, p.step);
    true
})?;
```

| path | what |
|---|---|
| `manifest.json` | `game` ("Bloody Roar (USA) SCUS-94199"), `format` (4), `source` (`bin` / `chd` / `iso`), `xa`, `cdda`, `content_id` (below), `fighters`, `stages`, `music`, `cd_audio`, `movies`, `files` (`[{path, size, crc32}]`, every file but the manifest) |
| `engine/tables.okt` | the executable's static tables and the game's text, by name (below) |
| `engine/<DISC PATH>` | every Form 1 file but the executable (only its tables are kept), verbatim (`XX/XX.ANM`, `XX/XX00.DAT`, `CMN/*` …) |
| `pictures/ART0/*.TIM`, `pictures/ART1/*.TIM` | the pictures, verbatim |
| `audio/sfx/<DISC PATH>.SND` | the sound banks, verbatim |
| `models/XX_human.glb`, `XX_beast.glb` | MIT, FOX, GRE, URI: skinned mesh in the bind pose, every costume a material variant, no animations |
| `stages/XX00.glb`, `BIG00.glb`, `SMA00.glb` | every stage scene with its sky band and floor |
| `audio/music/BGM0n_chK.xa` | each XA channel with sound (silent and repeated channels dropped) |
| `audio/cdda/track02.pcm` | the CD-DA track |
| `movies/NAME.bsf`, `movies/NAME.xa` | each movie: its MDEC frames and its CD-XA sound |

`engine/tables.okt` holds named, typed tables cut out of the executable, each with the address its first byte has
(`origin`), so a reader of the data needs no executable:

```text
"BR1TABLE" | u32 version = 1 | u32 count |
count × { u8 name_len, name (UTF-8), u8 elem, u16 rec_size, u32 count, u32 origin, u32 byte_len, bytes, pad to 4 }
```

(`elem`: 0 blob, 1 u8, 2 i16, 3 u16, 4 u32, 5 record of `rec_size` bytes; all numbers little-endian.) The tables
(`runtime::TABLES`): `math.random_bytes`, `math.atan`, `math.sin`, `math.atan_b`, `math.sqrt`, `math.rsqrt`; per
stage `stage.light`, `stage.walls`, `stage.colours`, `stage.ring_edge`, `stage.ring_hips`, `stage.shadow_floor`,
`stage.flap`; `input.default_buttons`; the effects' linked data `fx.data_a` and `fx.data_b`; `fighter.parts`;
`sound.volumes`; the CPU player's `ai.wait`, `ai.react`, `ai.guard`, `ai.recover`; `anim.common_clips` (the
executable's bytes the fighters' clips read; its `origin` is a file offset, not a RAM address) and
`anim.common_clips.ranges` (its ranges as (start, end) u32 file offset pairs); the scalar `image_len` (the
executable's length); and `text.strings`, the game's text (entries `key NUL text NUL`, trailing spaces and line ends
cut; `names.0`..`names.8` the fighters' names, `stages.0`..`stages.9` the stages' names, `final_stage`).

`content_id` is the CRC-32 of the lines `path:crc32\n` (CRC as 8 lower-case hex digits) of the executable (as the
disc has it: it is read, not written), `CMN/CMN.ACT` and the nine `XX/XX31.DAT` (under `engine/`, sorted by path), at
least 1: two conversions with the same id hold the same simulation data.

About 410 MB, 6–7 s from a BIN/CUE. An ISO image has no XA streams: no music, no movie sound (`xa: false`); without
the CD-DA track `cdda` is false.

- **`.xa` (v1):** `"BR2XA\0"`, u8 version 1, u8 the XA subheader's coding byte, u32 LE sector count N, u32 0, then
  N × 2304 bytes (each audio sector's 18 sound groups) up to and including the sector with the end-of-file bit.
- **`.bsf`:** `"BRBS"`, u16 width, u16 height, u32 frames, per frame u32 sector, u32 length and the bitstream.
- **`.pcm` (v1):** `"BR2PCM\0"`, u8 version 1, u32 LE rate (44100), u16 LE channels (2), u16 0, u32 LE frames N,
  u32 0, then N × channels 16-bit LE samples, interleaved (the track's bytes from its INDEX 01).

## Known gaps

- Clip names come from shared state names and command inputs (the game stores no move names); some clips are
  labelled by their motion. Root-motion yaw is unverified for partial turns; sounds the game picks at run time (hit
  impacts, random whooshes, voices) are listed without a file.
- The stage background is placed approximately (the game draws it as a 2D layer).
- VFX: particles attached to fighter bones, the special-move and beast effects, trails and the ring model's animation
  are not converted.
- `XX20.DAT` entries 2/3 (15 box parts per fighter) are exported unposed as `extra_a/b`.

## Development

| task | command |
|---|---|
| format | `cargo fmt` (`rustfmt.toml`, 120 columns) |
| lint | `cargo lint` (`clippy --all-targets -- -D warnings`) |
| tests | `cargo test` (`--features runtime` for the runtime data); the disc tests run when the disc is found (`BR1_DISC=path/to/folder-with-cue`) |
| coverage | `cargo cov` (HTML in `target/coverage/html`) or `cargo llvm-cov --summary-only` |
| security / licenses | `cargo audit`, `cargo deny check` (`deny.toml`) |
| fuzzing | `scripts/fuzz.sh [seconds]`: libFuzzer targets in `fuzz/` (cargo-fuzz and a nightly toolchain) |

CI (`.github/workflows/ci.yml`) runs fmt, clippy, the tests, cargo deny, cargo audit, coverage and a short run of
every fuzz target; it has no disc, so the disc tests skip there. `cargo test` installs the git hooks from
`.cargo-husky/hooks/` (pre-commit: fmt, clippy, tests; pre-push: audit, deny, a 50% line coverage gate over all features, which needs `BR1_DISC`).

## License

MIT ([LICENSE](LICENSE)). Third-party code and its licenses: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

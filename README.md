### This repository was entirely vibecoded, save for this readme.

*This was made in a few days using ChatGPT (Free), Google Antigravity (Free, Gemini 3.5 Flash [Medium]), and Claude Code (Pro, Sonnet 5 [High])*

---

**demo-voice-extract** -- A Counter-Strike: Source/Team Fortress 2 demo voice chat extractor for Linux. **(demo version 3, network protocol 24)**

Supports `vaudio_celt`, `vaudio_celt_high` and `steam` (Opus) voice codecs.

Each player's data will be output into separate .wav files, denoted by their SteamID3 (`U_1_xxxxxxx.wav`.). The audio will be in realtime, same as playing the demo ingame.

Tested demos:
 - SourceTV demos for CS:S (NiDE: `vaudio_celt`, UNLOZE: `steam`)
 - User demos for CS:S (NiDE: see above, JBlock: `vaudio_celt_high`, eGO: `steam`)
 - SourceTV demos for TF2 (Uncletopia, castaway.tf: `steam`)
 - **User demos for TF2 still not tested, but should work.**

The following resources were used:
 - `vaudio_celt_client.so` and `libtier0_client.so` 64bit libraries from Valve's Counter-Strike: Global Offensive
 - CELT's 0.11.1 headers from Xiph.org
 - [EloB's issue on demoinfocs-golang](https://github.com/markus-wa/demoinfocs-golang/issues/243)
 - [ericek111's CS:GO CELT decoder](https://gist.github.com/ericek111/abe5829f6e52e4b25b3b97a0efd0b22b)
 - [SpaceManiac/opus-rs](https://github.com/SpaceManiac/opus-rs)
 - [SizzlingStats/demboyz](https://github.com/SizzlingStats/demboyz)
 - [demostf/tf2-demo-parser](https://codeberg.org/demostf/parser)
 - [demostf/steam-audio-codec](https://codeberg.org/demostf/steam-audio-codec)
 - [Levi_OP's issue on steam-audio-codec](https://codeberg.org/demostf/steam-audio-codec/issues/1)


---

**Building**

```bash
# Pull parser and opus-rs
git submodule update --init --recursive
# Fetch CELT v0.11.1
cd lib/celt
curl -sSL http://downloads.xiph.org/releases/celt/celt-0.11.1.tar.gz | tar xzvf -
# Add stub for svc_SendTable event, parser never implemented and fails (JBlock demo failed)
cd ../parser
git apply ../../sendtable.patch
# Build!
cd ../..
cargo build --release
```

**Usage**

```bash
target/release/demo-voice-extract <path_to_demo.dem>
# OR
cargo run --release -- <path_to_demo.dem>
```

**Example**

```
~/demo-voice-extract$ curl -sSO https://demos.unloze.com/auto-20260710-161938-ze_tyranny2_v2_beta1_4-2.dem
~/demo-voice-extract$ target/release/demo-voice-extract auto-20260710-161938-ze_tyranny2_v2_beta1_4-2.dem
Demo header:
  demo type:   HL2DEMO
  version:     3
  protocol:    24
  server:      [EVENT] UNLOZE | Zombie Escape |
  nick:        SourceTV Demo
  map:         ze_tyranny2_v2_beta1_4
  game:        cstrike
  duration:    1911.25s
  ticks:       191125
  frames:      27759

Extracting the following players...
  Soviet Tornado [U:1:159406075] - steam (24000 Hz)
  Kriegsman [U:1:267328904] - steam (24000 Hz)
  Kriesley [U:1:281515870] - steam (24000 Hz)
  [B.] SFM [U:1:31231360] - steam (24000 Hz)
  vanster__ [U:1:31989906] - steam (24000 Hz)
  Pantcake [U:1:354780430] - steam (24000 Hz)
  Harraga [U:1:38050520] - steam (24000 Hz)
  MING [U:1:61926652] - steam (24000 Hz)
  Justin Case. [U:1:65984206] - steam (24000 Hz)
  jenz [U:1:69566635] - steam (24000 Hz)
  mini [U:1:859221657] - steam (24000 Hz)

~/demo-voice-extract$ ls *.wav
U_1_159406075.wav  U_1_31989906.wav   U_1_65984206.wav
U_1_267328904.wav  U_1_354780430.wav  U_1_69566635.wav
U_1_281515870.wav  U_1_38050520.wav   U_1_859221657.wav
U_1_31231360.wav   U_1_61926652.wav
```

use main_error::MainError;
use std::collections::HashMap;
use std::env;
use std::fs;

use tf_demo_parser::demo::data::DemoTick;
use tf_demo_parser::demo::header::Header;
use tf_demo_parser::demo::message::voice::VoiceInitMessage;
use tf_demo_parser::demo::message::Message;
use tf_demo_parser::demo::parser::MessageHandler;
use tf_demo_parser::{Demo, DemoParser, MessageType, ParserState};

mod celt;
use celt::{CeltDecoder, CeltVariant};

mod steam;
use steam::{SteamVoiceData, SteamVoiceDecoder};

/// Output mode, selected by an optional `--mix`/`--compact` flag (mutually exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Original behaviour: one .wav per player, padded with silence so playback stays
    /// aligned to the demo's real-time position.
    Normal,
    /// All players' voice streams overlaid (summed) into a single `downmix.wav`.
    Mix,
    /// One .wav per player like `Normal`, but without silence padding between (or before/
    /// after) transmissions -- just the decoded audio, back to back.
    Compact,
}

fn main() -> Result<(), MainError> {
    let args: Vec<_> = env::args().collect();

    let usage = format!("Usage: {} [--mix|--compact] <demo.dem>", args[0]);

    let mut mode = Mode::Normal;
    let mut demo_path: Option<&str> = None;

    for arg in &args[1..] {
        match arg.as_str() {
            "--mix" | "--compact" if mode != Mode::Normal => {
                eprintln!("Error: --mix and --compact are mutually exclusive");
                eprintln!("{usage}");
                std::process::exit(1);
            }
            "--mix" => mode = Mode::Mix,
            "--compact" => mode = Mode::Compact,
            _ if demo_path.is_some() => {
                eprintln!("{usage}");
                std::process::exit(1);
            }
            other => demo_path = Some(other),
        }
    }

    let Some(demo_path) = demo_path else {
        eprintln!("{usage}");
        std::process::exit(1);
    };

    let data = fs::read(demo_path)?;

    let demo = Demo::new(&data);
    let parser = DemoParser::new_with_analyser(demo.get_stream(), VoiceExtractor::new(mode));

    parser.parse()?;

    Ok(())
}

const SAMPLE_RATE: f64 = celt::OUTPUT_SAMPLE_RATE as f64;

struct VoiceExtractor {
    buffers: HashMap<String, Vec<(DemoTick, CeltVariant, Vec<u8>)>>,
    // Raw "steam" codec voice payloads, buffered separately from the CELT ones since they
    // decode completely differently (see `steam.rs`) and don't share a `CeltVariant` tag.
    steam_buffers: HashMap<String, Vec<(DemoTick, Vec<u8>)>>,
    client_steam_ids: HashMap<u8, String>,
    client_names: HashMap<u8, String>,
    // Name + codec for each speaker, keyed by the same resolved id used for `buffers`/
    // `steam_buffers`, captured at the time their voice data was received (not looked up
    // lazily at the end from `client_names`/`client_steam_ids`). Client slots get reused by
    // different players over a demo's lifetime, so resolving lazily at the end could
    // attribute a speaker's audio to whichever player happens to occupy their old slot by
    // the time parsing finishes.
    speaker_info: HashMap<String, (String, String)>,
    interval_per_tick: f32,
    total_duration: f32,
    last_init: Option<VoiceInitMessage>,
    header: Option<Header>,
    mode: Mode,
}

impl VoiceExtractor {
    fn new(mode: Mode) -> Self {
        Self {
            buffers: HashMap::new(),
            steam_buffers: HashMap::new(),
            client_steam_ids: HashMap::new(),
            client_names: HashMap::new(),
            speaker_info: HashMap::new(),
            interval_per_tick: 0.0,
            total_duration: 0.0,
            last_init: None,
            header: None,
            mode,
        }
    }

    /// Resolve a client slot to the id used to key `buffers`/`steam_buffers`/`speaker_info`.
    /// Must be called at the time voice data is received (not lazily), since client slots
    /// get reused by different players over a demo's lifetime.
    fn resolve_id(&self, client: u8) -> String {
        self.client_steam_ids
            .get(&client)
            .cloned()
            .unwrap_or_else(|| client.to_string())
    }

    fn buffer(&mut self, id: String) -> &mut Vec<(DemoTick, CeltVariant, Vec<u8>)> {
        self.buffers.entry(id).or_insert_with(Vec::new)
    }

    fn steam_buffer(&mut self, id: String) -> &mut Vec<(DemoTick, Vec<u8>)> {
        self.steam_buffers.entry(id).or_insert_with(Vec::new)
    }

    fn print_summary(&self) {
        if let Some(header) = &self.header {
            println!("Demo header:");
            println!("  demo type:   {}", header.demo_type);
            println!("  version:     {}", header.version);
            println!("  protocol:    {}", header.protocol);
            println!("  server:      {}", header.server);
            println!("  nick:        {}", header.nick);
            println!("  map:         {}", header.map);
            println!("  game:        {}", header.game);
            println!("  duration:    {:.2}s", header.duration);
            println!("  ticks:       {}", header.ticks);
            println!("  frames:      {}", header.frames);
            println!();
        }

        println!("Extracting the following players...");
        // Only speakers who actually have buffered voice data, keyed the same way
        // `buffers`/`steam_buffers` are, so this list can't drift from what actually gets
        // written out below.
        let mut ids: Vec<&String> = self
            .buffers
            .keys()
            .chain(self.steam_buffers.keys())
            .collect();
        ids.sort();
        ids.dedup();

        for id in ids {
            let (name, codec) = self
                .speaker_info
                .get(id)
                .cloned()
                .unwrap_or_else(|| ("<unknown>".to_string(), "<unknown>".to_string()));

            let codec_info = if codec == "steam" {
                // Sample rate isn't fixed for "steam" voice data (unlike CELT), so peek
                // it from this speaker's first `SampleRate` packet, the same way the
                // decode step below does.
                let sample_rate = self.steam_buffers.get(id).and_then(|messages| {
                    messages
                        .iter()
                        .find_map(|(_, raw)| SteamVoiceData::new(raw).ok()?.sample_rate())
                });
                match sample_rate {
                    Some(rate) => format!("steam ({rate} Hz)"),
                    None => "steam (unknown sample rate)".to_string(),
                }
            } else {
                format!("{codec} ({} Hz)", celt::OUTPUT_SAMPLE_RATE)
            };

            println!("  {name} {id} - {codec_info}");
        }
        println!();
    }
}

impl MessageHandler for VoiceExtractor {
    type Output = ();

    fn does_handle(message_type: MessageType) -> bool {
        matches!(
            message_type,
            MessageType::VoiceInit | MessageType::VoiceData
        )
    }

    fn handle_header(&mut self, header: &Header) {
        self.total_duration = header.duration;
        self.header = Some(header.clone());
    }

    fn handle_message(
        &mut self,
        message: &Message,
        tick: DemoTick,
        parser_state: &ParserState,
    ) {
        self.interval_per_tick = parser_state.demo_meta.interval_per_tick;

        match message {
            Message::VoiceInit(init) => {
                self.last_init = Some(init.clone());
            }
            Message::VoiceData(data) => {
                if let Some(init) = self.last_init.clone() {
                    let id = self.resolve_id(data.client);
                    let name = self
                        .client_names
                        .get(&data.client)
                        .cloned()
                        .unwrap_or_else(|| "<unknown>".to_string());
                    self.speaker_info
                        .entry(id.clone())
                        .or_insert_with(|| (name, init.codec.clone()));

                    if init.codec.as_str() == "steam" {
                        let payload = data
                            .data
                            .clone()
                            .read_bytes((data.length / 8) as usize)
                            .unwrap();

                        self.steam_buffer(id).push((tick, payload.into_owned()));
                    } else {
                        let variant = match init.codec.as_str() {
                            "vaudio_celt" => CeltVariant::Standard,
                            "vaudio_celt_high" => CeltVariant::High,
                            other => panic!("unsupported voice codec: {other}"),
                        };

                        let payload = data
                            .data
                            .clone()
                            .read_bytes((data.length / 8) as usize)
                            .unwrap();

                        self.buffer(id).push((tick, variant, payload.into_owned()));
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_string_entry(
        &mut self,
        table: &str,
        index: usize,
        entry: &tf_demo_parser::demo::packet::stringtable::StringTableEntry,
        _parser_state: &ParserState,
    ) {
        if table == "userinfo" {
            if let Ok(Some(user_info)) = tf_demo_parser::demo::data::UserInfo::parse_from_string_table(
                index as u16,
                entry.text.as_ref().map(|s| s.as_ref()),
                entry.extra_data.as_ref().map(|d| d.data.clone()),
            ) {
                if !user_info.player_info.steam_id.is_empty() {
                    let steam_id = convert_to_steam3(&user_info.player_info.steam_id);
                    self.client_steam_ids.insert(index as u8, steam_id);
                    self.client_names
                        .insert(index as u8, user_info.player_info.name.clone());
                }
            }
        }
    }

    fn into_output(mut self, _state: &ParserState) {
        self.print_summary();

        let interval_per_tick = self.interval_per_tick as f64;
        let total_duration = self.total_duration as f64;
        // `Mix` still needs each player's stream aligned to real time (otherwise overlaying
        // them is meaningless); only `Compact` skips the silence padding.
        let pad = self.mode != Mode::Compact;

        match self.mode {
            Mode::Normal | Mode::Compact => {
                for (steam_id, messages) in self.buffers.drain() {
                    let samples =
                        decode_celt_track(messages, interval_per_tick, total_duration, pad);
                    write_wav(
                        &format!("{}.wav", steam3_filename(&steam_id)),
                        celt::OUTPUT_SAMPLE_RATE,
                        &samples,
                    );
                }

                for (steam_id, messages) in self.steam_buffers.drain() {
                    let (sample_rate, samples) = decode_steam_track(
                        &steam_id,
                        messages,
                        interval_per_tick,
                        total_duration,
                        pad,
                    );
                    write_wav(
                        &format!("{}.wav", steam3_filename(&steam_id)),
                        sample_rate,
                        &samples,
                    );
                }
            }
            Mode::Mix => {
                let mut tracks: Vec<(u32, Vec<i16>)> = Vec::new();

                for (_steam_id, messages) in self.buffers.drain() {
                    let samples =
                        decode_celt_track(messages, interval_per_tick, total_duration, pad);
                    tracks.push((celt::OUTPUT_SAMPLE_RATE, samples));
                }

                for (steam_id, messages) in self.steam_buffers.drain() {
                    let (sample_rate, samples) = decode_steam_track(
                        &steam_id,
                        messages,
                        interval_per_tick,
                        total_duration,
                        pad,
                    );
                    tracks.push((sample_rate, samples));
                }

                if tracks.is_empty() {
                    eprintln!("warning: no voice data found, downmix.wav will be empty");
                }

                // A demo only ever uses one voice codec for every speaker (it's a
                // server-wide setting), so every track here already shares a sample rate.
                let target_rate = tracks
                    .first()
                    .map(|(rate, _)| *rate)
                    .unwrap_or(celt::OUTPUT_SAMPLE_RATE);

                let mut mix: Vec<i32> = Vec::new();
                for (_rate, samples) in tracks {
                    if mix.len() < samples.len() {
                        mix.resize(samples.len(), 0);
                    }
                    for (m, s) in mix.iter_mut().zip(samples.iter()) {
                        *m += *s as i32;
                    }
                }

                // Overlaying several speakers can exceed i16 range; clamp rather than
                // rescale so a single quiet demo doesn't get its overall volume changed.
                let samples: Vec<i16> = mix
                    .into_iter()
                    .map(|s| s.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
                    .collect();
                write_wav("downmix.wav", target_rate, &samples);
            }
        }
    }
}

/// Decode one player's buffered CELT voice messages into 16-bit mono PCM at
/// `celt::OUTPUT_SAMPLE_RATE`.
///
/// When `pad` is set, silence is inserted so each transmission lands at its real-time
/// position (matching demo playback) and the track is padded out to the demo's total
/// duration. When unset, decoded audio is simply concatenated back to back.
fn decode_celt_track(
    messages: Vec<(DemoTick, CeltVariant, Vec<u8>)>,
    interval_per_tick: f64,
    total_duration: f64,
    pad: bool,
) -> Vec<i16> {
    let mut decoder: Option<CeltDecoder> = None;
    let mut samples: Vec<i16> = Vec::new();

    for (tick, variant, raw) in messages {
        if pad {
            let target_sample =
                (u32::from(tick) as f64 * interval_per_tick * SAMPLE_RATE).round() as usize;
            if target_sample > samples.len() {
                samples.resize(target_sample, 0);
            }
        }

        let decoder = match &mut decoder {
            Some(decoder) if decoder.variant() == variant => decoder,
            _ => decoder.insert(CeltDecoder::new(variant)),
        };
        samples.extend_from_slice(&decoder.decode(&raw));
    }

    if pad {
        let total_samples = (total_duration * SAMPLE_RATE).round() as usize;
        if total_samples > samples.len() {
            samples.resize(total_samples, 0);
        }
    }

    samples
}

/// Decode one player's buffered "steam" voice messages into 16-bit mono PCM, returning the
/// sample rate declared by the stream itself along with the samples.
///
/// See `decode_celt_track` for what `pad` controls.
fn decode_steam_track(
    steam_id: &str,
    messages: Vec<(DemoTick, Vec<u8>)>,
    interval_per_tick: f64,
    total_duration: f64,
    pad: bool,
) -> (u32, Vec<i16>) {
    // Unlike CELT, "steam" voice data declares its own sample rate (typically 24000 Hz, but
    // the protocol allows several others), so it can't reuse `celt::OUTPUT_SAMPLE_RATE`. Peek
    // the first `SampleRate` packet found among this speaker's messages instead of assuming a
    // fixed rate.
    let sample_rate = messages
        .iter()
        .find_map(|(_, raw)| SteamVoiceData::new(raw).ok()?.sample_rate())
        .unwrap_or_else(|| {
            eprintln!(
                "warning: no SampleRate packet found for steam voice from {steam_id}, defaulting to 24000 Hz"
            );
            24000
        });
    let sample_rate_f64 = sample_rate as f64;

    // "steam" voice data carries its own internal silence run-lengths between Opus frames, so
    // (unlike CELT) a persistent decoder naturally reconstructs continuous, real-time-accurate
    // audio across consecutive messages from the same speaker on its own. But a single silence
    // run is capped at a u16 sample count (a few seconds at most), so it can't describe a long
    // stretch where this speaker sent no messages at all (e.g. not holding push-to-talk for a
    // while). The same `target_sample` padding CELT uses covers that case when `pad` is set: it
    // only pads when we're behind where we should be, so it won't double count the gaps the
    // decoder already filled in on its own.
    let mut decoder = SteamVoiceDecoder::new();
    let mut out_buffer = vec![0i16; sample_rate as usize * 10];
    let mut samples: Vec<i16> = Vec::new();

    for (tick, raw) in messages {
        if pad {
            let target_sample =
                (u32::from(tick) as f64 * interval_per_tick * sample_rate_f64).round() as usize;
            if target_sample > samples.len() {
                samples.resize(target_sample, 0);
            }
        }

        let steam_data = match SteamVoiceData::new(&raw) {
            Ok(steam_data) => steam_data,
            Err(e) => {
                eprintln!("warning: skipping malformed steam voice packet from {steam_id}: {e}");
                continue;
            }
        };
        match decoder.decode(steam_data, &mut out_buffer) {
            Ok(count) => samples.extend_from_slice(&out_buffer[..count]),
            Err(e) => eprintln!(
                "warning: skipping steam voice packet from {steam_id} that failed to decode: {e}"
            ),
        }
    }

    if pad {
        let total_samples = (total_duration * sample_rate_f64).round() as usize;
        if total_samples > samples.len() {
            samples.resize(total_samples, 0);
        }
    }

    (sample_rate as u32, samples)
}

fn write_wav(filename: &str, sample_rate: u32, samples: &[i16]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(filename, spec).unwrap();
    for &sample in samples {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

/// Turn a SteamID3 (e.g. `[U:1:19506566]`) into a string safe to use as a filename,
/// e.g. `U_1_19506566`.
fn steam3_filename(steam_id: &str) -> String {
    steam_id
        .chars()
        .filter(|c| *c != '[' && *c != ']')
        .map(|c| if c == ':' { '_' } else { c })
        .collect()
}

fn convert_to_steam3(steam_id: &str) -> String {
    let clean = steam_id.trim_matches('\0').trim();
    if clean.starts_with("[U:") {
        return clean.to_string();
    }
    if clean.starts_with("STEAM_") {
        let parts: Vec<&str> = clean.split(':').collect();
        if parts.len() == 3 {
            if let (Ok(y), Ok(z)) = (parts[1].parse::<u64>(), parts[2].parse::<u64>()) {
                let account_id = z * 2 + y;
                return format!("[U:1:{}]", account_id);
            }
        }
    }
    clean.to_string()
}

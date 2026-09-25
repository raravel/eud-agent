//! `audio_ffmpeg`: model-authored FFmpeg audio processing under an allowlist.
//!
//! The model writes the FFmpeg arguments; the app owns every input, output,
//! protocol, and file. Inputs appear only as `{inN}` right after `-i`, the one
//! output is the last argument written as `{out}/<name>`, every option and
//! filter must be on an audio-processing allowlist, and nothing is executed
//! until the whole argument list validates.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

pub const MAX_FFMPEG_INPUTS: usize = 8;
pub const MAX_FFMPEG_ARGS: usize = 128;
pub const MAX_FFMPEG_ARG_CHARS: usize = 4096;
pub const MAX_FFMPEG_ARGS_TOTAL_CHARS: usize = 16 * 1024;
pub const MAX_FFMPEG_OUTPUTS: usize = 128;
pub const MAX_FFMPEG_OUTPUT_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_OUTPUT_NAME_CHARS: usize = 64;
const MAX_MPQ_PATH_CHARS: usize = 260;
const MAX_AUDIO_REF_CHARS: usize = 64;
const MAX_STDERR_EXCERPT_CHARS: usize = 2000;
/// FFmpeg itself; probing the outputs shares what is left of [`FFMPEG_TOOL_BUDGET`].
pub const FFMPEG_TOOL_DEADLINE: Duration = Duration::from_secs(150);
/// The whole call, well under the 240-second tool-call bound.
pub const FFMPEG_TOOL_BUDGET: Duration = Duration::from_secs(200);
/// CPU seconds one FFmpeg run may use (also ends a child orphaned by a crash).
pub const FFMPEG_CPU_SECONDS: u64 = 160;
/// Memory one FFmpeg run may hold before it is stopped.
pub const MAX_FFMPEG_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Only one `audio_ffmpeg` job runs at a time in the process.
pub static FFMPEG_JOB: std::sync::Mutex<()> = std::sync::Mutex::new(());

const OUTPUT_EXTENSIONS: &[&str] = &["flac", "wav", "ogg"];
const OUTPUT_FORMATS: &[&str] = &["flac", "wav", "ogg", "segment"];
const SEGMENT_FORMATS: &[&str] = &["flac", "wav", "ogg"];
const AUDIO_CODECS: &[&str] = &["flac", "pcm_s16le", "pcm_s24le", "libvorbis"];
const SAMPLE_FORMATS: &[&str] = &[
    "u8", "s16", "s32", "flt", "dbl", "u8p", "s16p", "s32p", "fltp", "dblp",
];

/// Demuxers an input may be opened with, on every FFmpeg/FFprobe input
/// (`-format_whitelist`). Content probing would otherwise pick manifest
/// demuxers such as `dash`/`hls`/`concat` that open further files or URLs.
/// Each name is a real FFmpeg 8.1 demuxer name (`ogg` also covers Opus).
pub const INPUT_FORMAT_WHITELIST: &str =
    "wav,ogg,flac,mp3,aac,mov,mp4,m4a,3gp,3g2,mj2,aiff,asf,matroska,webm";

/// The shortest segment `-segment_time`/`-segment_times` may ask for, so a
/// tiny value cannot flood the output directory with files.
const MIN_SEGMENT_SECONDS: f64 = 0.05;

/// Windows device names that must never be an output file stem.
const RESERVED_DEVICE_STEMS: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Audio filters a filtergraph may name. Every entry only transforms sample
/// data already in the graph; none opens a file, a device, a URL, or a plugin,
/// and none runs commands. Sources (`anullsrc`, `sine`), whole-stream buffers
/// (`areverse`), unbounded padding (`apad`) and long echo buffers (`aecho`)
/// are left out because they can grow memory or output without bound; filters
/// whose options size a buffer or a fan-out are bounded by [`BOUNDED_FILTERS`].
pub const AUDIO_FILTERS: &[&str] = &[
    "atrim",
    "asetpts",
    "afade",
    "acrossfade",
    "volume",
    "concat",
    "amix",
    "amerge",
    "asplit",
    "aloop",
    "adelay",
    "atempo",
    "asetrate",
    "aresample",
    "aformat",
    "pan",
    "channelmap",
    "equalizer",
    "bass",
    "treble",
    "highpass",
    "lowpass",
    "bandpass",
    "acompressor",
    "alimiter",
    "loudnorm",
    "dynaudnorm",
    "compand",
    "silenceremove",
    "anull",
    "extrastereo",
    "stereotools",
    "crystalizer",
];

/// How one bounded filter option's value is checked.
#[derive(Debug, Clone, Copy)]
enum OptionValue {
    Integer(i64, i64),
    Decimal(f64, f64),
    OneOf(&'static [&'static str]),
    /// `|`-separated integers, each in range (e.g. `aformat` sample rates).
    IntegerList(i64, i64),
    /// `|`-separated millisecond decimals, each in range (`adelay`).
    DecimalList(f64, f64),
}

/// One option of a bounded filter: its accepted names (the first is the
/// positional slot's meaning) and its value check.
type BoundedOption = (&'static [&'static str], OptionValue);

/// Filters whose options size a buffer, a rate, or a fan-out. Only the listed
/// options are accepted, as `key=value` or in their positional order; any other
/// key, an extra positional value, or an out-of-range/unparseable value is
/// refused.
const BOUNDED_FILTERS: &[(&str, &[BoundedOption])] = &[
    (
        "aresample",
        &[(
            &["osr", "out_sample_rate"],
            OptionValue::Integer(8_000, 192_000),
        )],
    ),
    (
        "asetrate",
        &[(&["r", "sample_rate"], OptionValue::Integer(8_000, 192_000))],
    ),
    ("asplit", &[(&["outputs"], OptionValue::Integer(1, 16))]),
    (
        "concat",
        &[
            (&["n"], OptionValue::Integer(1, 8)),
            (&["v"], OptionValue::Integer(0, 0)),
            (&["a"], OptionValue::Integer(1, 2)),
        ],
    ),
    (
        "amix",
        &[
            (&["inputs"], OptionValue::Integer(1, 16)),
            (
                &["duration"],
                OptionValue::OneOf(&["longest", "shortest", "first"]),
            ),
            (&["dropout_transition"], OptionValue::Decimal(0.0, 60.0)),
            (&["normalize"], OptionValue::Integer(0, 1)),
        ],
    ),
    ("amerge", &[(&["inputs"], OptionValue::Integer(1, 16))]),
    (
        "aloop",
        &[
            (&["loop"], OptionValue::Integer(0, 100)),
            (&["size"], OptionValue::Integer(0, 1_920_000)),
            (&["start"], OptionValue::Integer(0, 1_000_000_000)),
        ],
    ),
    ("atempo", &[(&["tempo"], OptionValue::Decimal(0.5, 4.0))]),
    (
        "adelay",
        &[
            (&["delays"], OptionValue::DecimalList(0.0, 60_000.0)),
            (&["all"], OptionValue::Integer(0, 1)),
        ],
    ),
    (
        "aformat",
        &[
            (
                &["sample_fmts", "f"],
                OptionValue::OneOf(&[
                    "u8", "s16", "s32", "flt", "dbl", "u8p", "s16p", "s32p", "fltp", "dblp",
                ]),
            ),
            (
                &["sample_rates", "r"],
                OptionValue::IntegerList(8_000, 192_000),
            ),
            (
                &["channel_layouts", "cl"],
                OptionValue::OneOf(&["mono", "stereo"]),
            ),
        ],
    ),
];

/// Options valid before an `-i` (they apply to that input).
const INPUT_OPTIONS: &[&str] = &["-ss", "-t", "-to", "-sseof", "-stream_loop", "-itsoffset"];
/// Options valid after the last `-i` (they apply to the one output).
const OUTPUT_OPTIONS: &[&str] = &[
    "-ss",
    "-t",
    "-to",
    "-af",
    "-filter:a",
    "-filter_complex",
    "-map",
    "-ac",
    "-ar",
    "-c:a",
    "-codec:a",
    "-acodec",
    "-b:a",
    "-q:a",
    "-sample_fmt",
    "-aframes",
    "-shortest",
    "-f",
    "-segment_time",
    "-segment_times",
    "-segment_format",
    "-reset_timestamps",
    "-segment_start_number",
    "-vn",
    "-sn",
    "-dn",
    "-map_metadata",
    "-y",
];
/// Options that may appear anywhere because FFmpeg treats them as global.
const GLOBAL_OPTIONS: &[&str] = &["-filter_complex", "-y"];
const FLAG_OPTIONS: &[&str] = &["-shortest", "-vn", "-sn", "-dn", "-y"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FfmpegInput {
    AudioRef(String),
    MpqPath(String),
}

/// A validated `audio_ffmpeg` call: nothing here has touched a file yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegPlan {
    pub inputs: Vec<FfmpegInput>,
    args: Vec<String>,
    output_name: String,
}

impl FfmpegPlan {
    /// The final FFmpeg argv: the app's prefix, `-protocol_whitelist file` and
    /// `-format_whitelist` on every input, `{inN}` replaced by
    /// `input_paths[N]`, `-vn -sn -dn -map_chapters -1` on the output, and
    /// `{out}/<name>` replaced by `output_dir/<name>`.
    pub fn command_args(&self, input_paths: &[PathBuf], output_dir: &Path) -> Vec<String> {
        let mut command = vec![
            "-nostdin".to_string(),
            "-hide_banner".to_string(),
            "-v".to_string(),
            "error".to_string(),
        ];
        let last = self.args.len() - 1;
        let mut index = 0;
        while index < last {
            let arg = &self.args[index];
            if arg == "-i" {
                let input = input_placeholder(&self.args[index + 1])
                    .expect("validated -i values are input placeholders");
                command.extend([
                    "-protocol_whitelist".to_string(),
                    "file".to_string(),
                    "-format_whitelist".to_string(),
                    INPUT_FORMAT_WHITELIST.to_string(),
                    "-i".to_string(),
                    input_paths[input].to_string_lossy().into_owned(),
                ]);
                index += 2;
                continue;
            }
            command.push(arg.clone());
            index += 1;
        }
        command.extend(
            ["-vn", "-sn", "-dn", "-map_chapters", "-1"]
                .into_iter()
                .map(str::to_string),
        );
        command.push(
            output_dir
                .join(&self.output_name)
                .to_string_lossy()
                .into_owned(),
        );
        command
    }
}

/// Parse and validate the tool arguments without executing anything.
pub fn parse_request(args: &Value) -> Result<FfmpegPlan, String> {
    let inputs = args
        .get("inputs")
        .and_then(Value::as_array)
        .ok_or_else(|| usage("inputs 배열이 필요합니다."))?;
    if inputs.is_empty() || inputs.len() > MAX_FFMPEG_INPUTS {
        return Err(usage(&format!(
            "inputs는 1~{MAX_FFMPEG_INPUTS}개여야 합니다 (현재 {}개).",
            inputs.len()
        )));
    }
    let inputs = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| parse_input(index, input))
        .collect::<Result<Vec<_>, _>>()?;
    let args = args
        .get("args")
        .and_then(Value::as_array)
        .ok_or_else(|| usage("args 문자열 배열이 필요합니다."))?
        .iter()
        .enumerate()
        .map(|(index, arg)| {
            arg.as_str()
                .map(str::to_string)
                .ok_or_else(|| usage(&format!("args[{index}]는 문자열이어야 합니다.")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let output_name = validate_args(&args, inputs.len())?;
    Ok(FfmpegPlan {
        inputs,
        args,
        output_name,
    })
}

fn parse_input(index: usize, input: &Value) -> Result<FfmpegInput, String> {
    let object = input
        .as_object()
        .filter(|object| object.len() == 1)
        .ok_or_else(|| {
            usage(&format!(
                "inputs[{index}]는 {{\"audioRef\": ...}} 또는 {{\"mpqPath\": ...}} 중 정확히 하나여야 합니다."
            ))
        })?;
    if let Some(audio_ref) = object.get("audioRef").and_then(Value::as_str) {
        if audio_ref.is_empty()
            || audio_ref.len() > MAX_AUDIO_REF_CHARS
            || !audio_ref
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(usage(&format!(
                "inputs[{index}].audioRef가 올바르지 않습니다. [audio attachments]나 이전 audio_ffmpeg 결과의 audio-N을 그대로 쓰세요."
            )));
        }
        return Ok(FfmpegInput::AudioRef(audio_ref.to_string()));
    }
    if let Some(mpq_path) = object.get("mpqPath").and_then(Value::as_str) {
        if mpq_path.is_empty()
            || mpq_path.chars().count() > MAX_MPQ_PATH_CHARS
            || mpq_path.chars().any(char::is_control)
        {
            return Err(usage(&format!(
                "inputs[{index}].mpqPath가 올바르지 않습니다. map_sound_list가 돌려준 mpqPath를 그대로 쓰세요."
            )));
        }
        return Ok(FfmpegInput::MpqPath(mpq_path.to_string()));
    }
    Err(usage(&format!(
        "inputs[{index}]는 {{\"audioRef\": 문자열}} 또는 {{\"mpqPath\": 문자열}}이어야 합니다."
    )))
}

fn usage(reason: &str) -> String {
    format!(
        "audio_ffmpeg 사용 오류: {reason} 실행하지 않았습니다. 형식: inputs=[{{audioRef}}|{{mpqPath}}], args=[..., \"-i\", \"{{in0}}\", ..., \"{{out}}/<이름>.flac|wav|ogg\"]."
    )
}

/// Validate the model's FFmpeg arguments; returns the output file name.
fn validate_args(args: &[String], input_count: usize) -> Result<String, String> {
    if args.is_empty() || args.len() > MAX_FFMPEG_ARGS {
        return Err(usage(&format!(
            "args는 1~{MAX_FFMPEG_ARGS}개여야 합니다 (현재 {}개).",
            args.len()
        )));
    }
    let mut total = 0usize;
    for (index, arg) in args.iter().enumerate() {
        let chars = arg.chars().count();
        total += chars;
        if chars > MAX_FFMPEG_ARG_CHARS {
            return Err(usage(&format!(
                "args[{index}]가 {MAX_FFMPEG_ARG_CHARS}자를 넘습니다."
            )));
        }
        if arg.chars().any(char::is_control) {
            return Err(usage(&format!("args[{index}]에 제어 문자가 있습니다.")));
        }
    }
    if total > MAX_FFMPEG_ARGS_TOTAL_CHARS {
        return Err(usage(&format!(
            "args 전체 길이가 {MAX_FFMPEG_ARGS_TOTAL_CHARS}자를 넘습니다."
        )));
    }

    let last = args.len() - 1;
    let output_name = output_name(&args[last])?;
    let last_input = args[..last].iter().rposition(|arg| arg == "-i");
    let Some(last_input) = last_input else {
        return Err(usage(
            "입력이 없습니다. \"-i\", \"{in0}\"처럼 입력을 지정하세요.",
        ));
    };

    let mut used_inputs = vec![false; input_count];
    let mut format: Option<String> = None;
    let mut index = 0;
    while index < last {
        let arg = args[index].as_str();
        if arg == "-i" {
            let value = args
                .get(index + 1)
                .filter(|_| index + 1 < last)
                .ok_or_else(|| usage("\"-i\" 뒤에 {inN}이 필요합니다."))?;
            let input = input_placeholder(value).ok_or_else(|| {
                usage(&format!(
                    "args[{}] '{value}': \"-i\" 값은 {{in0}}..{{in{}}} 자리표시자만 쓸 수 있습니다. 파일 경로, URL, 프로토콜은 거부됩니다.",
                    index + 1,
                    input_count - 1
                ))
            })?;
            if input >= input_count {
                return Err(usage(&format!(
                    "args[{}] '{value}': inputs는 {input_count}개뿐입니다.",
                    index + 1
                )));
            }
            if std::mem::replace(&mut used_inputs[input], true) {
                return Err(usage(&format!(
                    "args[{}] '{value}': 같은 입력을 두 번 열 수 없습니다. 필터에서 asplit을 쓰세요.",
                    index + 1
                )));
            }
            index += 2;
            continue;
        }
        if contains_placeholder(arg) {
            return Err(usage(&format!(
                "args[{index}] '{arg}': {{inN}}은 \"-i\" 바로 뒤에만, {{out}}/<이름>은 마지막 인자에만 쓸 수 있습니다."
            )));
        }
        if !arg.starts_with('-') || arg == "-" {
            return Err(usage(&format!(
                "args[{index}] '{arg}': 옵션 값이 아닌 위치 인자입니다. 출력은 마지막 인자 {{out}}/<이름> 하나뿐입니다."
            )));
        }
        if arg.starts_with("-/") {
            return Err(usage(&format!(
                "args[{index}] '{arg}': 파일에서 옵션 값을 읽는 '-/옵션' 형식은 허용되지 않습니다. 값을 직접 쓰세요."
            )));
        }
        let input_side = index < last_input;
        let allowed = if input_side {
            INPUT_OPTIONS.contains(&arg) || GLOBAL_OPTIONS.contains(&arg)
        } else {
            OUTPUT_OPTIONS.contains(&arg)
        };
        if !allowed {
            let side = if input_side {
                format!("입력 옵션(\"-i\" 앞): {}", INPUT_OPTIONS.join(" "))
            } else {
                format!("출력 옵션(마지막 \"-i\" 뒤): {}", OUTPUT_OPTIONS.join(" "))
            };
            return Err(usage(&format!(
                "args[{index}] '{arg}'는 이 위치에서 허용되지 않는 옵션입니다. 허용 {side}"
            )));
        }
        if FLAG_OPTIONS.contains(&arg) {
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .filter(|_| index + 1 < last)
            .ok_or_else(|| usage(&format!("args[{index}] '{arg}' 뒤에 값이 필요합니다.")))?;
        if contains_placeholder(value) {
            return Err(usage(&format!(
                "args[{}] '{value}': {{inN}}은 \"-i\" 바로 뒤에만, {{out}}/<이름>은 마지막 인자에만 쓸 수 있습니다.",
                index + 1
            )));
        }
        validate_option_value(arg, value)
            .map_err(|reason| usage(&format!("args[{}] {arg} '{value}': {reason}", index + 1)))?;
        if arg == "-f" {
            format = Some(value.clone());
        }
        index += 2;
    }
    if let Some(missing) = used_inputs.iter().position(|used| !used) {
        return Err(usage(&format!(
            "inputs[{missing}]를 쓰지 않았습니다. 모든 입력을 \"-i\", \"{{in{missing}}}\"로 여세요."
        )));
    }
    let segmented = format.as_deref() == Some("segment");
    let patterned = output_name.contains('%');
    if segmented && !patterned {
        return Err(usage(
            "-f segment 출력 이름에는 %03d 같은 번호 패턴이 필요합니다. 예: {out}/part%03d.flac",
        ));
    }
    if patterned && !segmented {
        return Err(usage(
            "출력 이름의 % 번호 패턴은 -f segment와 함께만 쓸 수 있습니다.",
        ));
    }
    Ok(output_name)
}

fn input_placeholder(value: &str) -> Option<usize> {
    let digits = value.strip_prefix("{in")?.strip_suffix('}')?;
    if digits.is_empty() || digits.len() > 1 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn contains_placeholder(value: &str) -> bool {
    value.contains("{in") || value.contains("{out}")
}

fn output_name(last: &str) -> Result<String, String> {
    let name = last.strip_prefix("{out}/").ok_or_else(|| {
        usage(&format!(
            "마지막 인자 '{last}'는 {{out}}/<이름> 형식이어야 합니다. 예: {{out}}/part%03d.flac"
        ))
    })?;
    let valid_chars = name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'%' | b'.' | b'-'));
    let starts_alphanumeric = name
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric());
    if name.is_empty()
        || name.len() > MAX_OUTPUT_NAME_CHARS
        || !valid_chars
        || !starts_alphanumeric
        || name.contains("..")
    {
        return Err(usage(&format!(
            "출력 이름 '{name}'이 올바르지 않습니다. 영숫자로 시작하고 [A-Za-z0-9_%.-]만 쓰는 {MAX_OUTPUT_NAME_CHARS}자 이하 파일 이름이어야 하며 경로 구분자와 '..'는 쓸 수 없습니다."
        )));
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if RESERVED_DEVICE_STEMS.contains(&stem.as_str()) {
        return Err(usage(&format!(
            "출력 이름 '{name}'은 Windows 장치 이름({stem})이라 쓸 수 없습니다. 다른 이름을 쓰세요."
        )));
    }
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if !OUTPUT_EXTENSIONS.contains(&extension.as_str()) {
        return Err(usage(&format!(
            "출력 이름 '{name}'의 확장자는 flac, wav, ogg 중 하나여야 합니다."
        )));
    }
    if let Some(start) = name.find('%') {
        let rest = &name[start + 1..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let pattern_ok = rest[digits..].starts_with('d')
            && digits <= 2
            && (digits == 0 || rest.starts_with('0'))
            && !rest[digits + 1..].contains('%');
        if !pattern_ok {
            return Err(usage(&format!(
                "출력 이름 '{name}'의 번호 패턴은 %d 또는 %03d 형식 하나만 쓸 수 있습니다."
            )));
        }
    }
    Ok(name.to_string())
}

fn validate_option_value(option: &str, value: &str) -> Result<(), String> {
    let one_of = |allowed: &[&str]| {
        if allowed.contains(&value) {
            Ok(())
        } else {
            Err(format!("허용 값: {}", allowed.join(", ")))
        }
    };
    match option {
        "-ss" | "-t" | "-to" | "-itsoffset" => time_value(value, false),
        "-segment_time" => {
            time_value(value, false)?;
            if time_seconds(value) < MIN_SEGMENT_SECONDS {
                return Err(format!(
                    "조각 길이는 {MIN_SEGMENT_SECONDS}초 이상이어야 합니다."
                ));
            }
            Ok(())
        }
        "-sseof" => time_value(value, true),
        "-segment_times" => {
            if value.split(',').count() > MAX_FFMPEG_OUTPUTS {
                return Err(format!("시점은 {MAX_FFMPEG_OUTPUTS}개 이하여야 합니다."));
            }
            let mut previous = 0.0;
            for time in value.split(',') {
                time_value(time, false)?;
                let seconds = time_seconds(time);
                if seconds - previous < MIN_SEGMENT_SECONDS {
                    return Err(format!(
                        "시점은 {MIN_SEGMENT_SECONDS}초 이상 간격으로 커져야 합니다 ('{time}')."
                    ));
                }
                previous = seconds;
            }
            Ok(())
        }
        "-stream_loop" => bounded_integer(value, 0, 100)
            .map_err(|_| "반복 횟수는 0~100 정수여야 합니다 (무한 반복 -1은 거부).".to_string()),
        "-af" | "-filter:a" | "-filter_complex" => validate_filtergraph(value),
        "-map" => map_value(value),
        "-ac" => bounded_integer(value, 1, 8),
        "-ar" => bounded_integer(value, 8_000, 192_000),
        "-c:a" | "-codec:a" | "-acodec" => one_of(AUDIO_CODECS),
        "-b:a" => {
            let digits = value.strip_suffix('k').unwrap_or(value);
            if !digits.is_empty()
                && digits.len() <= 7
                && digits.bytes().all(|byte| byte.is_ascii_digit())
            {
                Ok(())
            } else {
                Err("비트레이트는 192k 또는 192000 형식이어야 합니다.".to_string())
            }
        }
        "-q:a" => decimal(value, true).map_err(|_| "품질 값은 숫자여야 합니다.".to_string()),
        "-sample_fmt" => one_of(SAMPLE_FORMATS),
        "-aframes" | "-segment_start_number" => bounded_integer(value, 0, 100_000_000),
        "-f" => one_of(OUTPUT_FORMATS),
        "-segment_format" => one_of(SEGMENT_FORMATS),
        "-reset_timestamps" => one_of(&["0", "1"]),
        "-map_metadata" => {
            if value == "-1" {
                Ok(())
            } else {
                bounded_integer(value, 0, (MAX_FFMPEG_INPUTS - 1) as i64)
            }
        }
        _ => Err("허용되지 않는 옵션입니다.".to_string()),
    }
}

fn bounded_integer(value: &str, minimum: i64, maximum: i64) -> Result<(), String> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let parsed = (!digits.is_empty()
        && digits.len() <= 12
        && digits.bytes().all(|byte| byte.is_ascii_digit()))
    .then(|| value.parse::<i64>().ok())
    .flatten();
    match parsed {
        Some(parsed) if (minimum..=maximum).contains(&parsed) => Ok(()),
        _ => Err(format!("{minimum}~{maximum} 정수여야 합니다.")),
    }
}

fn decimal(value: &str, allow_negative: bool) -> Result<(), ()> {
    let body = if allow_negative {
        value.strip_prefix('-').unwrap_or(value)
    } else {
        value
    };
    let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
    let digits_ok = |part: &str| part.len() <= 9 && part.bytes().all(|byte| byte.is_ascii_digit());
    if !whole.is_empty()
        && digits_ok(whole)
        && digits_ok(fraction)
        && (!body.contains('.') || !fraction.is_empty())
    {
        Ok(())
    } else {
        Err(())
    }
}

/// `SS[.frac]`, `MM:SS[.frac]` or `HH:MM:SS[.frac]`, optionally negative for `-sseof`.
fn time_value(value: &str, allow_negative: bool) -> Result<(), String> {
    let error = || "시간은 4.032 또는 00:02:18.5 형식이어야 합니다.".to_string();
    let body = if allow_negative {
        value.strip_prefix('-').unwrap_or(value)
    } else {
        value
    };
    let parts = body.split(':').collect::<Vec<_>>();
    if parts.len() > 3 || value.len() > 32 {
        return Err(error());
    }
    let (seconds, clock) = parts.split_last().ok_or_else(error)?;
    decimal(seconds, false).map_err(|_| error())?;
    if clock
        .iter()
        .any(|part| part.is_empty() || part.len() > 2 || !part.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(error());
    }
    Ok(())
}

/// Seconds of a value [`time_value`] accepted (non-negative form).
fn time_seconds(value: &str) -> f64 {
    value.split(':').fold(0.0, |total, part| {
        total * 60.0 + part.parse::<f64>().unwrap_or(0.0)
    })
}

fn map_value(value: &str) -> Result<(), String> {
    let error = || "-map 값은 0:a, 1:a:0 같은 입력 오디오 또는 [label]이어야 합니다.".to_string();
    if let Some(label) = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return if valid_label(label) {
            Ok(())
        } else {
            Err(error())
        };
    }
    let body = value.strip_suffix('?').unwrap_or(value);
    let mut parts = body.split(':');
    let input = parts.next().unwrap_or_default();
    if input.is_empty() || input.len() > 1 || !input.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(error());
    }
    match (parts.next(), parts.next(), parts.next()) {
        (None, None, None) | (Some("a"), None, None) => Ok(()),
        (Some("a"), Some(stream), None)
            if !stream.is_empty()
                && stream.len() <= 2
                && stream.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            Ok(())
        }
        _ => Err(error()),
    }
}

/// A filtergraph link label: a name (`mix`) or an input stream (`0:a`, `1:a:0`).
fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 32
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':'))
}

/// Conservative filtergraph check. Quoting and escaping are NOT interpreted:
/// the graph is split on every `,` and `;`, link labels and `@instance` names
/// are stripped, and every filter name must be on [`AUDIO_FILTERS`]. Quotes,
/// backslashes and `/` (FFmpeg's `/option=file` loads a value from a file) are
/// refused outright, so a false rejection is possible but a hidden filter or a
/// file read is not.
pub fn validate_filtergraph(graph: &str) -> Result<(), String> {
    if graph.trim().is_empty() {
        return Err("필터그래프가 비어 있습니다.".to_string());
    }
    if let Some(bad) = graph
        .chars()
        .find(|character| matches!(character, '\'' | '"' | '\\' | '/') || character.is_control())
    {
        return Err(format!(
            "필터그래프에 '{bad}' 문자를 쓸 수 없습니다 (따옴표, 이스케이프, 파일 경로 거부). 나눗셈 대신 소수(예: volume=0.5)를 쓰세요."
        ));
    }
    for segment in graph.split([',', ';']) {
        let mut rest = segment.trim();
        while let Some(stripped) = rest.strip_prefix('[') {
            let (label, after) = stripped
                .split_once(']')
                .ok_or_else(|| format!("필터 '{segment}'의 [label]이 닫히지 않았습니다."))?;
            if !valid_label(label) {
                return Err(format!(
                    "필터 '{segment}'의 label '[{label}]'은 영숫자, _, :만 쓸 수 있습니다."
                ));
            }
            rest = after.trim_start();
        }
        let end = rest.find(['=', '[']).unwrap_or(rest.len());
        let (name, arguments) = rest.split_at(end);
        let name = name.trim();
        let name = name.split_once('@').map_or(name, |(name, _)| name);
        if !AUDIO_FILTERS.contains(&name) {
            return Err(format!(
                "필터 '{name}'는 허용되지 않습니다. 허용 필터: {}",
                AUDIO_FILTERS.join(", ")
            ));
        }
        if let Some((_, options)) = BOUNDED_FILTERS.iter().find(|(bounded, _)| *bounded == name) {
            let values = arguments
                .strip_prefix('=')
                .map(|values| values.split('[').next().unwrap_or_default())
                .unwrap_or_default();
            validate_bounded_options(name, values, options)?;
        }
        let mut trailing = match arguments.find('[') {
            Some(start) => &arguments[start..],
            None => "",
        };
        while let Some(stripped) = trailing.strip_prefix('[') {
            let (label, after) = stripped
                .split_once(']')
                .ok_or_else(|| format!("필터 '{segment}'의 [label]이 닫히지 않았습니다."))?;
            if !valid_label(label) {
                return Err(format!(
                    "필터 '{segment}'의 label '[{label}]'은 영숫자, _, :만 쓸 수 있습니다."
                ));
            }
            trailing = after.trim_start();
        }
        if !trailing.is_empty() {
            return Err(format!(
                "필터 '{segment}'의 출력 label 뒤에 남은 문자 '{trailing}'가 있습니다."
            ));
        }
    }
    Ok(())
}

fn validate_bounded_options(
    filter: &str,
    values: &str,
    options: &[BoundedOption],
) -> Result<(), String> {
    let names = || {
        options
            .iter()
            .map(|(names, _)| names.join("|"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if values.trim().is_empty() {
        return Ok(());
    }
    let mut named = false;
    for (position, part) in values.split(':').enumerate() {
        let (option, value) = match part.split_once('=') {
            Some((key, value)) => {
                named = true;
                let option = options
                    .iter()
                    .find(|(names, _)| names.contains(&key.trim()))
                    .ok_or_else(|| {
                        format!(
                            "필터 {filter}의 옵션 '{key}'는 허용되지 않습니다. 허용: {}",
                            names()
                        )
                    })?;
                (option, value.trim())
            }
            None if !named && position < options.len() => (&options[position], part.trim()),
            None => {
                return Err(format!(
                "필터 {filter}의 값 '{part}'를 해석할 수 없습니다. key=value로 쓰세요. 허용: {}",
                names()
            ))
            }
        };
        let (names, check) = option;
        let key = names[0];
        let error = |range: String| format!("필터 {filter}의 {key}='{value}': {range}");
        let decimal = |text: &str| {
            decimal(text, false)
                .ok()
                .and_then(|_| text.parse::<f64>().ok())
        };
        match *check {
            OptionValue::Integer(minimum, maximum) => {
                bounded_integer(value, minimum, maximum).map_err(error)?
            }
            OptionValue::Decimal(minimum, maximum) => {
                if !decimal(value).is_some_and(|parsed| (minimum..=maximum).contains(&parsed)) {
                    return Err(error(format!("{minimum}~{maximum} 숫자여야 합니다.")));
                }
            }
            OptionValue::OneOf(allowed) => {
                if !allowed.contains(&value) {
                    return Err(error(format!("허용 값: {}", allowed.join(", "))));
                }
            }
            OptionValue::IntegerList(minimum, maximum) => {
                for item in value.split('|') {
                    bounded_integer(item, minimum, maximum).map_err(error)?;
                }
            }
            OptionValue::DecimalList(minimum, maximum) => {
                for item in value.split('|') {
                    if !decimal(item).is_some_and(|parsed| (minimum..=maximum).contains(&parsed)) {
                        return Err(error(format!(
                            "각 값은 {minimum}~{maximum} 밀리초 숫자여야 합니다."
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

/// One regular file FFmpeg wrote into the per-call output directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegOutputFile {
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
}

/// Every regular file in `dir`, sorted by name. Symlinks, directories and
/// anything else, an empty result, more than [`MAX_FFMPEG_OUTPUTS`] files, a
/// file over `max_file_bytes`, or more than [`MAX_FFMPEG_OUTPUT_TOTAL_BYTES`]
/// in total are refused.
pub fn collect_outputs(dir: &Path, max_file_bytes: u64) -> Result<Vec<FfmpegOutputFile>, String> {
    let entries =
        fs::read_dir(dir).map_err(|_| "audio_ffmpeg 출력 폴더를 읽을 수 없습니다.".to_string())?;
    let mut outputs = Vec::new();
    let mut total = 0u64;
    for entry in entries {
        let entry = entry.map_err(|_| "audio_ffmpeg 출력 폴더를 읽을 수 없습니다.".to_string())?;
        let file_type = entry
            .file_type()
            .map_err(|_| "audio_ffmpeg 출력 파일 형식을 확인할 수 없습니다.".to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !file_type.is_file() {
            return Err(format!(
                "audio_ffmpeg 출력 '{name}'이 일반 파일이 아닙니다 (링크/폴더 거부)."
            ));
        }
        let bytes = entry
            .metadata()
            .map_err(|_| "audio_ffmpeg 출력 파일 크기를 확인할 수 없습니다.".to_string())?
            .len();
        if bytes == 0 || bytes > max_file_bytes {
            return Err(format!(
                "audio_ffmpeg 출력 '{name}' 크기({bytes} bytes)가 1..={max_file_bytes} 범위를 벗어났습니다."
            ));
        }
        total = total.saturating_add(bytes);
        outputs.push(FfmpegOutputFile {
            name,
            path: entry.path(),
            bytes,
        });
        if outputs.len() > MAX_FFMPEG_OUTPUTS {
            return Err(format!(
                "audio_ffmpeg 출력 파일이 {MAX_FFMPEG_OUTPUTS}개를 넘습니다. 조각 길이를 늘리세요."
            ));
        }
    }
    if total > MAX_FFMPEG_OUTPUT_TOTAL_BYTES {
        return Err(format!(
            "audio_ffmpeg 출력 합계가 {MAX_FFMPEG_OUTPUT_TOTAL_BYTES} bytes를 넘습니다."
        ));
    }
    if outputs.is_empty() {
        return Err("audio_ffmpeg가 출력 파일을 만들지 않았습니다.".to_string());
    }
    outputs.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(outputs)
}

/// Entry count and total size of what is directly in `dir` (used while
/// FFmpeg runs).
pub fn directory_usage(dir: &Path) -> (usize, u64) {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .fold((0, 0), |(count, bytes), entry| {
            let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            (count + 1, bytes.saturating_add(size))
        })
}

/// A bounded stderr excerpt with the app's own paths replaced by the
/// placeholders the model wrote.
pub fn stderr_excerpt(stderr: &[u8], input_paths: &[PathBuf], output_dir: &Path) -> String {
    let mut text = String::from_utf8_lossy(stderr).into_owned();
    let output = output_dir.to_string_lossy().into_owned();
    if !output.is_empty() {
        text = text.replace(&output, "{out}");
    }
    for (index, path) in input_paths.iter().enumerate() {
        let path = path.to_string_lossy().into_owned();
        if !path.is_empty() {
            text = text.replace(&path, &format!("{{in{index}}}"));
        }
    }
    let text = text.trim();
    let count = text.chars().count();
    if count <= MAX_STDERR_EXCERPT_CHARS {
        return text.to_string();
    }
    let tail = text
        .chars()
        .skip(count - MAX_STDERR_EXCERPT_CHARS)
        .collect::<String>();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan(args: &[&str]) -> Result<FfmpegPlan, String> {
        parse_request(&json!({"inputs": [{"audioRef": "audio-1"}], "args": args}))
    }

    #[test]
    fn segment_recipe_is_accepted_and_rendered_with_app_owned_paths() {
        let plan = plan(&[
            "-i",
            "{in0}",
            "-f",
            "segment",
            "-segment_time",
            "4.032",
            "-reset_timestamps",
            "1",
            "-c:a",
            "flac",
            "{out}/part%03d.flac",
        ])
        .unwrap();
        let input = PathBuf::from("/tmp/request/in-0.ogg");
        let out = PathBuf::from("/tmp/request/ffmpeg-1");
        let command = plan.command_args(std::slice::from_ref(&input), &out);
        assert_eq!(
            command,
            vec![
                "-nostdin",
                "-hide_banner",
                "-v",
                "error",
                "-protocol_whitelist",
                "file",
                "-format_whitelist",
                INPUT_FORMAT_WHITELIST,
                "-i",
                input.to_str().unwrap(),
                "-f",
                "segment",
                "-segment_time",
                "4.032",
                "-reset_timestamps",
                "1",
                "-c:a",
                "flac",
                "-vn",
                "-sn",
                "-dn",
                "-map_chapters",
                "-1",
                out.join("part%03d.flac").to_str().unwrap(),
            ]
        );
    }

    #[test]
    fn two_inputs_with_trims_filters_and_maps_are_accepted() {
        let plan = parse_request(&json!({
            "inputs": [{"audioRef": "audio-1"}, {"mpqPath": "staredit\\wav\\bgm.ogg"}],
            "args": [
                "-ss", "1.5", "-t", "00:00:10", "-i", "{in1}",
                "-sseof", "-3", "-i", "{in0}",
                "-filter_complex", "[0:a]afade=t=out:st=8:d=2[a0];[1:a]volume@v=0.5,aresample=osr=44100[a1];[a0][a1]acrossfade=d=1[mix]",
                "-map", "[mix]", "-ac", "2", "-ar", "44100", "-c:a", "libvorbis", "-q:a", "4",
                "-map_metadata", "-1", "-vn", "-y", "{out}/mix.ogg"
            ],
        }))
        .unwrap();
        assert_eq!(
            plan.inputs,
            vec![
                FfmpegInput::AudioRef("audio-1".to_string()),
                FfmpegInput::MpqPath("staredit\\wav\\bgm.ogg".to_string())
            ]
        );
        let command = plan.command_args(
            &[PathBuf::from("/a/zero.ogg"), PathBuf::from("/a/one.ogg")],
            Path::new("/a/out"),
        );
        let inputs = command
            .windows(6)
            .filter(|window| {
                window[0] == "-protocol_whitelist"
                    && window[1] == "file"
                    && window[2] == "-format_whitelist"
                    && window[3] == INPUT_FORMAT_WHITELIST
                    && window[4] == "-i"
            })
            .map(|window| window[5].clone())
            .collect::<Vec<_>>();
        assert_eq!(inputs, vec!["/a/one.ogg", "/a/zero.ogg"]);
    }

    #[test]
    fn inputs_must_be_exactly_one_known_source_shape() {
        for inputs in [
            json!([]),
            json!([{}]),
            json!([{"audioRef": "audio-1", "mpqPath": "a.ogg"}]),
            json!([{"path": "/etc/passwd"}]),
            json!([{"audioRef": "../audio-1"}]),
            json!([{"mpqPath": ""}]),
            json!((0..9)
                .map(|_| json!({"audioRef": "audio-1"}))
                .collect::<Vec<_>>()),
        ] {
            let error =
                parse_request(&json!({"inputs": inputs, "args": ["-i", "{in0}", "{out}/a.flac"]}))
                    .unwrap_err();
            assert!(error.contains("실행하지 않았습니다"), "{error}");
        }
    }

    #[test]
    fn input_placeholders_appear_only_after_dash_i_and_open_every_input_once() {
        for (args, expected) in [
            (vec!["-i", "/etc/passwd", "{out}/a.flac"], "자리표시자만"),
            (vec!["-i", "file:{in0}", "{out}/a.flac"], "자리표시자만"),
            (vec!["-i", "{in1}", "{out}/a.flac"], "1개뿐"),
            (vec!["-i", "{in0}", "-i", "{in0}", "{out}/a.flac"], "두 번"),
            (
                vec!["-i", "{in0}", "-af", "volume={in0}", "{out}/a.flac"],
                "바로 뒤에만",
            ),
            (vec!["-c:a", "flac", "{out}/a.flac"], "입력이 없습니다"),
            (vec!["{in0}", "{out}/a.flac"], "입력이 없습니다"),
        ] {
            let error = plan(&args).unwrap_err();
            assert!(error.contains(expected), "{args:?}: {error}");
        }
        let unused = parse_request(&json!({
            "inputs": [{"audioRef": "audio-1"}, {"audioRef": "audio-2"}],
            "args": ["-i", "{in0}", "{out}/a.flac"],
        }))
        .unwrap_err();
        assert!(unused.contains("inputs[1]"), "{unused}");
    }

    #[test]
    fn output_must_be_last_and_a_plain_audio_file_name() {
        for args in [
            vec!["-i", "{in0}", "{out}/a.flac", "-y"],
            vec!["-i", "{in0}", "/tmp/a.flac"],
            vec!["-i", "{in0}", "{out}/../a.flac"],
            vec!["-i", "{in0}", "{out}/sub/a.flac"],
            vec!["-i", "{in0}", "{out}/a\\b.flac"],
            vec!["-i", "{in0}", "{out}/-a.flac"],
            vec!["-i", "{in0}", "{out}/a.mp3"],
            vec!["-i", "{in0}", "{out}/a"],
            vec!["-i", "{in0}", "{out}/한글.flac"],
            vec!["-i", "{in0}", "{out}/a%s.flac"],
            vec!["-i", "{in0}", "{out}/a.flac", "{out}/b.flac"],
            vec!["-i", "{in0}", "{out}/a%03d.flac"],
            vec!["-i", "{in0}", "-f", "segment", "{out}/a.flac"],
            vec!["-i", "{in0}", "-f", "segment", "{out}/a%03d%03d.flac"],
        ] {
            assert!(plan(&args).is_err(), "{args:?} must be rejected");
        }
    }

    #[test]
    fn options_outside_the_allowlist_are_rejected_before_execution() {
        for args in [
            vec!["-i", "{in0}", "-i", "{in0}", "{out}/a.flac"],
            vec![
                "-protocol_whitelist",
                "file,http",
                "-i",
                "{in0}",
                "{out}/a.flac",
            ],
            vec!["-i", "{in0}", "-protocol_blacklist", "http", "{out}/a.flac"],
            vec!["-i", "{in0}", "-filter_script", "x.txt", "{out}/a.flac"],
            vec![
                "-i",
                "{in0}",
                "-filter_complex_script",
                "x.txt",
                "{out}/a.flac",
            ],
            vec!["-i", "{in0}", "-/af", "x.txt", "{out}/a.flac"],
            vec!["-i", "{in0}", "-/filter_complex", "x.txt", "{out}/a.flac"],
            vec!["-dump_attachment:t:0", "x", "-i", "{in0}", "{out}/a.flac"],
            vec!["-i", "{in0}", "-attach", "x", "{out}/a.flac"],
            vec!["-report", "-i", "{in0}", "{out}/a.flac"],
            vec!["-i", "{in0}", "-progress", "x", "{out}/a.flac"],
            vec!["-i", "{in0}", "-vstats_file", "x", "{out}/a.flac"],
            vec!["-i", "{in0}", "-passlogfile", "x", "{out}/a.flac"],
            vec!["-i", "{in0}", "-sdp_file", "x", "{out}/a.flac"],
            vec!["-i", "{in0}", "-f", "tee", "{out}/a.flac"],
            vec!["-i", "{in0}", "-f", "hls", "{out}/a.flac"],
            vec![
                "-i",
                "{in0}",
                "-f",
                "segment",
                "-segment_list",
                "x",
                "{out}/a%03d.flac",
            ],
            vec!["-f", "lavfi", "-i", "{in0}", "{out}/a.flac"],
            vec!["-i", "{in0}", "-v", "debug", "{out}/a.flac"],
            vec!["-i", "{in0}", "-c:a", "libmp3lame", "{out}/a.flac"],
            vec!["-i", "{in0}", "-c:v", "png", "{out}/a.flac"],
            vec!["-i", "{in0}", "-segment_format", "mp4", "{out}/a.flac"],
            vec!["-stream_loop", "-1", "-i", "{in0}", "{out}/a.flac"],
            vec!["-i", "{in0}", "-ss", "1;2", "{out}/a.flac"],
            vec!["-i", "{in0}", "-map", "0:v", "{out}/a.flac"],
            vec!["-i", "{in0}", "-ac", "99", "{out}/a.flac"],
            vec!["-i", "{in0}", "extra", "{out}/a.flac"],
            vec!["-i", "{in0}", "-c:a", "{out}/a.flac"],
            vec!["-af", "volume=0.5", "-i", "{in0}", "{out}/a.flac"],
        ] {
            let error = plan(&args).unwrap_err();
            assert!(error.contains("실행하지 않았습니다"), "{args:?}: {error}");
        }
    }

    #[test]
    fn filtergraphs_name_only_allowlisted_audio_filters() {
        for graph in [
            "atrim=start=0:end=4.032,asetpts=PTS-STARTPTS",
            "volume@gain=0.5,afade=t=in:d=1",
            "[0:a][1:a]concat=n=2:v=0:a=1[out]",
            "[0:a]asplit=2[a][b];[a]volume=0.5[r];[b][r]amix=inputs=2:duration=first",
            "pan=stereo|c0=c0|c1=c1",
            "aresample=44100,asetrate=r=48000,atempo=1.25",
            "aloop=loop=3:size=44100,adelay=delays=1500|1500:all=1",
            "aformat=sample_fmts=s16:sample_rates=44100|48000:channel_layouts=stereo",
            "concat=n=8:v=0:a=1,aresample",
        ] {
            validate_filtergraph(graph).unwrap_or_else(|error| panic!("{graph}: {error}"));
        }
        for graph in [
            "amovie=secret.wav",
            "movie=x.mp4",
            "[in]movie=x[out]",
            "volume=1,amovie=x",
            "volume=1;sendcmd=f=cmds.txt",
            "ladspa=file=x",
            "lv2=p=x",
            "subtitles=x.srt",
            "afir",
            "volume=1/2",
            "/volume=gain.txt",
            "volume=/gain.txt",
            "'movie'=x",
            "mo\\vie=x",
            "volume=\"1\"",
            "",
            "[bad label]volume=1",
            "[a volume=1",
            "volume=1[out]junk",
            "Volume=1",
            // Whole-stream buffers, sources, and unbounded padding.
            "areverse",
            "anullsrc=r=192000:cl=7.1,atrim=duration=1",
            "sine=f=440",
            "[0:a]apad,aresample=192000",
            "aecho=0.8:0.9:90000:0.3",
            // Out-of-range or unparseable sizes, rates, and fan-outs.
            "aresample=384000",
            "aresample=osr=7999",
            "aresample=44100:resampler=soxr",
            "asetrate=sample_rate=1e6",
            "asplit=17",
            "concat=n=9:v=0:a=1",
            "concat=n=2:v=1:a=1",
            "amix=inputs=17",
            "amix=weights=1 1",
            "aloop=loop=-1:size=1000",
            "aloop=loop=2:size=1920001",
            "atempo=100",
            "adelay=60001",
            "adelay=1500S",
            "aformat=sample_rates=384000",
            "aformat=channel_layouts=7.1",
            "asplit=outputs=2:extra",
        ] {
            assert!(
                validate_filtergraph(graph).is_err(),
                "{graph} must be rejected"
            );
        }
        let injected = plan(&["-i", "{in0}", "-af", "amovie=x", "{out}/a.flac"]).unwrap_err();
        assert!(injected.contains("amovie"), "{injected}");
    }

    #[test]
    fn segment_times_are_floored_and_increasing_and_device_names_are_refused() {
        let segment = |option: &str, value: &str| {
            plan(&[
                "-i",
                "{in0}",
                "-f",
                "segment",
                option,
                value,
                "{out}/p%03d.wav",
            ])
        };
        assert!(segment("-segment_time", "0.05").is_ok());
        assert!(segment("-segment_time", "0.001").is_err());
        assert!(segment("-segment_time", "0").is_err());
        assert!(segment("-segment_times", "4.032,8.064,00:00:12.096").is_ok());
        assert!(segment("-segment_times", "0.01").is_err());
        assert!(segment("-segment_times", "4,4.02").is_err());
        assert!(segment("-segment_times", "8,4").is_err());
        for name in [
            "CON.wav",
            "nul.flac",
            "Com1.ogg",
            "lpt9.part.wav",
            "aux.wav",
        ] {
            let error = plan(&["-i", "{in0}", &format!("{{out}}/{name}")]).unwrap_err();
            assert!(error.contains("장치 이름"), "{name}: {error}");
        }
        assert!(plan(&["-i", "{in0}", "{out}/console.wav"]).is_ok());
    }

    #[test]
    fn output_collection_takes_only_bounded_regular_files() {
        let dir = std::env::temp_dir().join(format!("eud-ffmpeg-collect-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        assert!(collect_outputs(&dir, 16)
            .unwrap_err()
            .contains("만들지 않았습니다"));
        fs::write(dir.join("part001.flac"), b"bb").unwrap();
        fs::write(dir.join("part000.flac"), b"a").unwrap();
        let outputs = collect_outputs(&dir, 16).unwrap();
        assert_eq!(
            outputs
                .iter()
                .map(|output| output.name.as_str())
                .collect::<Vec<_>>(),
            vec!["part000.flac", "part001.flac"]
        );
        assert_eq!(outputs[1].bytes, 2);
        assert_eq!(directory_usage(&dir), (2, 3));
        assert!(collect_outputs(&dir, 1).unwrap_err().contains("범위"));
        fs::write(dir.join("empty.flac"), b"").unwrap();
        assert!(collect_outputs(&dir, 16).is_err());
        fs::remove_file(dir.join("empty.flac")).unwrap();
        fs::create_dir(dir.join("nested")).unwrap();
        assert!(collect_outputs(&dir, 16).unwrap_err().contains("일반 파일"));
        fs::remove_dir(dir.join("nested")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("part000.flac"), dir.join("link.flac")).unwrap();
            assert!(collect_outputs(&dir, 16).unwrap_err().contains("일반 파일"));
            fs::remove_file(dir.join("link.flac")).unwrap();
        }
        for index in 0..=MAX_FFMPEG_OUTPUTS {
            fs::write(dir.join(format!("many{index:03}.flac")), b"x").unwrap();
        }
        assert!(collect_outputs(&dir, 16)
            .unwrap_err()
            .contains("개를 넘습니다"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn stderr_excerpt_hides_app_paths_and_is_bounded() {
        let input = PathBuf::from("/secret/app/in-0.ogg");
        let out = PathBuf::from("/secret/app/ffmpeg-1");
        let excerpt = stderr_excerpt(
            b"/secret/app/in-0.ogg: Invalid data\n/secret/app/ffmpeg-1/a.flac: failed",
            std::slice::from_ref(&input),
            &out,
        );
        assert_eq!(excerpt, "{in0}: Invalid data\n{out}/a.flac: failed");
        let long = "x".repeat(MAX_STDERR_EXCERPT_CHARS * 2);
        let bounded = stderr_excerpt(long.as_bytes(), &[], &out);
        assert_eq!(bounded.chars().count(), MAX_STDERR_EXCERPT_CHARS + 1);
    }
}

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::bootstrap::ManagedFfmpegPaths;

use super::E2BgmError;

const PROCESS_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_SEGMENTS: usize = 512;
const MAX_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;

pub(super) struct ConvertedTrack {
    pub segments: Vec<Vec<u8>>,
}

pub(super) struct ConvertTrackRequest<'a> {
    pub tools: &'a ManagedFfmpegPaths,
    pub temp: &'a Path,
    pub track: u32,
    pub source: &'a [u8],
    pub sound_interval: u32,
    pub is_cancelled: &'a dyn Fn() -> bool,
}

pub(super) fn convert_track(
    request: ConvertTrackRequest<'_>,
) -> Result<ConvertedTrack, E2BgmError> {
    let ConvertTrackRequest {
        tools,
        temp,
        track,
        source,
        sound_interval,
        is_cancelled,
    } = request;
    let track_dir = temp.join(format!("track-{track}"));
    fs::create_dir(&track_dir)
        .map_err(|error| fatal(format!("cannot create BGM temp: {error}")))?;
    let input = track_dir.join("source-input");
    let whole = track_dir.join("whole.ogg");
    fs::write(&input, source).map_err(|error| fatal(format!("cannot write BGM input: {error}")))?;
    run(
        &tools.ffmpeg,
        &[
            "-nostdin".into(),
            "-v".into(),
            "error".into(),
            "-i".into(),
            input.into_os_string(),
            "-map_metadata".into(),
            "-1".into(),
            "-fflags".into(),
            "+bitexact".into(),
            "-flags:a".into(),
            "+bitexact".into(),
            "-vn".into(),
            "-c:a".into(),
            "libvorbis".into(),
            "-y".into(),
            whole.clone().into_os_string(),
        ],
        is_cancelled,
    )?;
    let segment_seconds = format!("{:.6}", f64::from(sound_interval) * 1.05);
    let pattern = track_dir.join(format!("M{track}_%d.ogg"));
    run(
        &tools.ffmpeg,
        &[
            "-nostdin".into(),
            "-v".into(),
            "error".into(),
            "-i".into(),
            whole.into_os_string(),
            "-f".into(),
            "segment".into(),
            "-fflags".into(),
            "+bitexact".into(),
            "-segment_format_options".into(),
            "fflags=+bitexact".into(),
            "-segment_time".into(),
            segment_seconds.into(),
            "-c".into(),
            "copy".into(),
            "-y".into(),
            pattern.into_os_string(),
        ],
        is_cancelled,
    )?;
    let paths = contiguous_segments(&track_dir, track)?;
    let mut segments = Vec::with_capacity(paths.len());
    for path in paths {
        probe_ogg(tools, &path, is_cancelled)?;
        let metadata = fs::metadata(&path)
            .map_err(|error| fatal(format!("cannot inspect BGM segment: {error}")))?;
        if metadata.len() == 0 || metadata.len() > MAX_SEGMENT_BYTES {
            return Err(fatal("BGM segment size is invalid"));
        }
        let bytes =
            fs::read(&path).map_err(|error| fatal(format!("cannot read BGM segment: {error}")))?;
        if !bytes.starts_with(b"OggS") {
            return Err(fatal("BGM segment is not an OGG stream"));
        }
        segments.push(bytes);
    }
    Ok(ConvertedTrack { segments })
}

pub(super) fn contiguous_segments(
    directory: &Path,
    track: u32,
) -> Result<Vec<PathBuf>, E2BgmError> {
    let prefix = format!("M{track}_");
    let mut indexed = BTreeMap::new();
    for entry in fs::read_dir(directory)
        .map_err(|error| fatal(format!("cannot enumerate BGM segments: {error}")))?
    {
        let entry = entry.map_err(|error| fatal(format!("cannot read BGM segment: {error}")))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(value) = name
            .strip_prefix(&prefix)
            .and_then(|value| value.strip_suffix(".ogg"))
        else {
            continue;
        };
        let index = value
            .parse::<u32>()
            .map_err(|_| fatal("BGM segment name has a non-numeric index"))?;
        if indexed.insert(index, entry.path()).is_some() || indexed.len() > MAX_SEGMENTS {
            return Err(fatal("BGM segment inventory is invalid"));
        }
    }
    if indexed.is_empty() {
        return Err(fatal("BGM conversion produced no segments"));
    }
    let mut paths = Vec::with_capacity(indexed.len());
    for (expected, (actual, path)) in (0_u32..).zip(indexed) {
        if actual != expected {
            return Err(fatal("BGM segment inventory contains a gap"));
        }
        paths.push(path);
    }
    Ok(paths)
}

fn probe_ogg(
    tools: &ManagedFfmpegPaths,
    path: &Path,
    is_cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<(), E2BgmError> {
    let output = run(
        &tools.ffprobe,
        &[
            "-v".into(),
            "error".into(),
            "-select_streams".into(),
            "a:0".into(),
            "-show_entries".into(),
            "stream=codec_name".into(),
            "-of".into(),
            "default=noprint_wrappers=1:nokey=1".into(),
            path.as_os_str().to_owned(),
        ],
        is_cancelled,
    )?;
    if String::from_utf8_lossy(&output).trim() != "vorbis" {
        return Err(fatal("BGM segment is not a validated Vorbis OGG stream"));
    }
    Ok(())
}

fn run(
    executable: &Path,
    args: &[std::ffi::OsString],
    is_cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<Vec<u8>, E2BgmError> {
    if is_cancelled() {
        return Err(E2BgmError::Cancelled);
    }
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| fatal(format!("cannot start managed audio tool: {error}")))?;
    let started = Instant::now();
    loop {
        if is_cancelled() {
            terminate(&mut child)?;
            return Err(E2BgmError::Cancelled);
        }
        if started.elapsed() > PROCESS_TIMEOUT {
            terminate(&mut child)?;
            return Err(fatal("managed audio conversion timed out"));
        }
        match child
            .try_wait()
            .map_err(|error| fatal(format!("cannot wait for managed audio tool: {error}")))?
        {
            Some(status) => {
                let stdout = read_pipe(child.stdout.take(), "stdout")?;
                if !status.success() {
                    return Err(fatal("managed audio conversion failed"));
                }
                return Ok(stdout);
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn terminate(child: &mut Child) -> Result<(), E2BgmError> {
    child
        .kill()
        .map_err(|error| fatal(format!("cannot terminate managed audio tool: {error}")))?;
    child
        .wait()
        .map_err(|error| fatal(format!("cannot reap managed audio tool: {error}")))?;
    Ok(())
}

fn read_pipe<R: Read>(pipe: Option<R>, name: &str) -> Result<Vec<u8>, E2BgmError> {
    let mut pipe = pipe.ok_or_else(|| fatal(format!("managed audio tool has no {name}")))?;
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)
        .map_err(|error| fatal(format!("cannot read managed audio {name}: {error}")))?;
    Ok(bytes)
}

fn fatal(message: impl Into<String>) -> E2BgmError {
    E2BgmError::Fatal(super::invalid(message))
}

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::config::DataDirs;

pub(super) const INPUT_MAP: &str = "E:/proj/eud/test.scx";
pub(super) const EUDDRAFT: &str = "E:/proj/eud/euddraft0.10.2.5/euddraft.exe";

#[derive(Clone, Copy)]
pub(super) enum TileMode {
    Memory,
    File,
}

pub(super) struct Package {
    pub root: PathBuf,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub moved: PathBuf,
    pub unavailable: PathBuf,
    pub dirs: DataDirs,
    pub originals: Vec<(PathBuf, String)>,
}

impl Package {
    pub fn create(owner: &OwnedRoot, tag: &str, mode: TileMode) -> Self {
        let root = owner.0.join(format!("package-{tag}"));
        for child in ["Map", "Resource", "Sound"] {
            fs::create_dir_all(root.join(child)).unwrap();
        }
        fs::copy(INPUT_MAP, root.join("Map/source.scx")).unwrap();
        fs::copy(tone_path(), root.join("Sound/track.ogg")).unwrap();
        write_resources(&root, mode);
        let source = root.join(format!("resources-{tag}.e2p"));
        fs::write(&source, source_text(mode)).unwrap();
        let dirs = DataDirs::from_bases(
            &owner.0.join(format!("roaming-{tag}")),
            &owner.0.join(format!("local-{tag}")),
        );
        dirs.ensure_dirs().unwrap();
        link_installed_ffmpeg(&dirs);
        let originals = files(&root)
            .into_iter()
            .map(|path| {
                let hash = sha256_file(&path);
                (path, hash)
            })
            .collect();
        Self {
            source,
            destination: owner.0.join(format!("destination-{tag}")),
            moved: owner.0.join(format!("moved-{tag}")),
            unavailable: owner.0.join(format!("unavailable-{tag}")),
            root,
            dirs,
            originals,
        }
    }

    pub fn assert_originals(&self, root: &Path) {
        for (old, hash) in &self.originals {
            let relative = old.strip_prefix(&self.root).unwrap();
            assert_eq!(sha256_file(&root.join(relative)), *hash, "{relative:?}");
        }
    }
}

pub(super) struct OwnedRoot(pub PathBuf);

impl OwnedRoot {
    pub fn new(tag: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "eud-agent-e2-resource-relocation-{tag}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for OwnedRoot {
    fn drop(&mut self) {
        let metadata = fs::symlink_metadata(&self.0).unwrap();
        assert!(!crate::memory::is_reparse_point(&metadata));
        fs::remove_dir_all(&self.0).unwrap();
    }
}

pub(super) fn sha256_file(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn tone_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/isom/tests/fixtures/tone.ogg")
}

fn link_installed_ffmpeg(dirs: &DataDirs) {
    let installed = DataDirs::from_bases(
        Path::new(&std::env::var("APPDATA").unwrap()),
        Path::new(&std::env::var("LOCALAPPDATA").unwrap()),
    );
    let tools = crate::bootstrap::resolve_managed_ffmpeg(&installed).unwrap();
    for tool in [tools.ffmpeg, tools.ffprobe] {
        let target = dirs.bin_dir().join(tool.file_name().unwrap());
        if fs::hard_link(&tool, &target).is_err() {
            fs::copy(tool, target).unwrap();
        }
    }
}

fn write_resources(root: &Path, mode: TileMode) {
    fs::write(root.join("Resource/scene.bin"), [2, 0, 0, 0]).unwrap();
    fs::write(root.join("Resource/B.eps"), "function plugin_b() {}\n").unwrap();
    fs::write(
        root.join("Resource/a.py"),
        "from eudplib import *\n\ndef onPluginStart():\n    pass\n",
    )
    .unwrap();
    fs::write(
        root.join("Resource/stat_txt.tbl"),
        b"\x03\x00\x08\x00\x0d\x00\x1a\x00zero\0external-one\0tail\0",
    )
    .unwrap();
    fs::write(root.join("Resource/aiscript.bin"), b"AISC-relocated").unwrap();
    fs::write(root.join("Resource/iscript.bin"), b"ISCR-relocated").unwrap();
    if matches!(mode, TileMode::File) {
        fs::write(root.join("Resource/tile.bmp"), indexed_bmp()).unwrap();
    }
}

fn indexed_bmp() -> Vec<u8> {
    let mut bmp = vec![0_u8; 0x436 + 32 * 32];
    let bmp_len = bmp.len() as u32;
    bmp[0..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&bmp_len.to_le_bytes());
    bmp[10..14].copy_from_slice(&(0x436_u32).to_le_bytes());
    bmp[14..18].copy_from_slice(&(40_u32).to_le_bytes());
    bmp[18..22].copy_from_slice(&(32_i32).to_le_bytes());
    bmp[22..26].copy_from_slice(&(32_i32).to_le_bytes());
    bmp[26..28].copy_from_slice(&(1_u16).to_le_bytes());
    bmp[28..30].copy_from_slice(&(8_u16).to_le_bytes());
    for (index, byte) in bmp[0x436..].iter_mut().enumerate() {
        *byte = (index % 2) as u8;
    }
    bmp
}

fn source_text(mode: TileMode) -> String {
    let tile = match mode {
        TileMode::Memory => format!(
            "ProjectTileUseFile : False\r\nProjectTileSetDataCount : 2\r\nProjectTileSetDataTileSetNum0 : 73\r\nProjectTileSetDataisMaker0 : True\r\nProjectTileSetDataTileSetData0 : {}\r\nProjectTileSetDataTileSetNum1 : 74\r\nProjectTileSetDataisMaker1 : False\r\nProjectTileSetDataTileSetData1 : {}\r\n",
            vec!["1"; 1024].join(","), vec!["2"; 1024].join(",")
        ),
        TileMode::File => "ProjectTileUseFile : True\r\nProjectTileSetFileName : Resource/tile.bmp\r\n".into(),
    };
    format!(
        "S_ProjectSET\r\nVersion : 0.17.9.7\r\nProjectName : Relocated Resources\r\nInputMap : Map/source.scx\r\nOutputMap : Map/output.scx\r\nUsedSetting0 : False\r\nUsedSetting1 : False\r\nUsedSetting2 : True\r\nUsedSetting3 : True\r\nUsedSetting4 : True\r\nUsedSetting5 : False\r\nUsedSetting6 : False\r\nUsedSetting7 : True\r\nUsedSetting8 : True\r\nE_ProjectSET\r\nS_BinEditSET\r\nBin0 : pointer=0x006D5EC8,width=4,value=0x00000002,PlayerRace=0\r\nDataFile : Resource/scene.bin\r\nE_BinEditSET\r\nS_SoundPlayerSET\r\nSoundinterval : 1\r\nSound/track.ogg\r\nE_SoundPlayerSET\r\nS_TileSET\r\n{tile}E_TileSET\r\nS_PluginSET\r\nB : Resource/B.eps\r\nBEnabled : True\r\na : Resource/a.py\r\naEnabled : True\r\nE_PluginSET\r\nS_FileManagerSET\r\nstat_txt : Resource/stat_txt.tbl\r\nAIscript : Resource/aiscript.bin\r\niscript : Resource/iscript.bin\r\nE_FileManagerSET\r\n"
    )
}

fn files(root: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    collect_files(root, &mut result);
    result.sort();
    result
}

fn collect_files(root: &Path, result: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_files(&path, result);
        } else if path.is_file() {
            result.push(path);
        }
    }
}

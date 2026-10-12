use std::collections::BTreeMap;

use super::super::{E2BgmPlan, E2CompiledSources};

pub(super) fn render(plan: &E2BgmPlan, tct_helpers: String) -> E2CompiledSources {
    let mut source = String::from(
        "import e2.tct as tct;\n\nconst musicLastTime = [0, 0, 0, 0, 0, 0, 0, 0];\nconst musicFrame = [0, 0, 0, 0, 0, 0, 0, 0];\nconst musicnum = [0, 0, 0, 0, 0, 0, 0, 0];\nconst musicflag = [0, 0, 0, 0, 0, 0, 0, 0];\nconst musicisplay = [0, 0, 0, 0, 0, 0, 0, 0];\n\nfunction Player() {\n    var currentid = getcurpl();\n    for (var i = 0; i < 8; i++) {\n        if (musicisplay[i] == 1 && musicflag[i] != 2) {\n            var playtime = musicLastTime[i] - dwread_epd(EPD(0x51CE8C));\n            var LastFrame = 0;\n            const music_No = musicnum[i];\n",
    );
    for track in &plan.tracks {
        source.push_str(&format!(
            "            if (music_No == {}) {{ LastFrame = {}; }}\n",
            track.music_number,
            track.segments.len()
        ));
    }
    let threshold = u64::from(plan.sound_interval) * 1_050 - 70;
    source.push_str(&format!(
        "            if (musicFrame[i] < LastFrame) {{\n                if (playtime > {threshold}) {{\n                    tct.makeText('M');\n                    const music_name = musicnum[i];\n                    tct.addText(music_name);\n                    tct.addText('_');\n                    const music_frame = musicFrame[i];\n                    tct.addText(music_frame);\n                    tct.addText('.ogg');\n                    SetCurrentPlayer(i);\n"
    ));
    for track in &plan.tracks {
        source.push_str(&format!(
            "                    if (music_name == {}) {{\n",
            track.music_number
        ));
        for segment in &track.segments {
            source.push_str(&format!(
                "                        if (music_frame == {}) {{ PlayWAV('{}'); }}\n",
                segment.index, segment.alias
            ));
        }
        source.push_str("                    }\n");
    }
    let initial = u64::from(plan.sound_interval) * 1_050;
    source.push_str(&format!(
        "                    musicFrame[i] += 1;\n                    musicLastTime[i] = dwread_epd(EPD(0x51CE8C));\n                }}\n            }} else {{\n                if (musicflag[i] == 1) {{\n                    musicFrame[i] = 0;\n                    musicLastTime[i] = dwread_epd(EPD(0x51CE8C));\n                }} else if (musicflag[i] == 0) {{\n                    musicflag[i] = 2;\n                }}\n            }}\n        }}\n    }}\n    SetCurrentPlayer(currentid);\n}}\n\nfunction parsePlayer(tplayer) {{\n    if (tplayer < 8) {{ return tplayer; }}\n    else {{ return getcurpl(); }}\n}}\n\nfunction Play(tplayer, BGMNum, flag) {{\n    var p = parsePlayer(tplayer);\n    if (flag == 0 && musicflag[p] == 0) {{\n        if (BGMNum != musicnum[p]) {{\n            musicLastTime[p] = dwread_epd(EPD(0x51CE8C)) + {initial};\n            musicFrame[p] = 0;\n        }}\n        musicnum[p] = BGMNum;\n        musicflag[p] = flag;\n        musicisplay[p] = 1;\n    }} else {{\n        musicLastTime[p] = dwread_epd(EPD(0x51CE8C)) + {initial};\n        musicFrame[p] = 0;\n        musicnum[p] = BGMNum;\n        musicflag[p] = flag;\n        musicisplay[p] = 1;\n    }}\n}}\n\nfunction Stop(tplayer) {{\n    var p = parsePlayer(tplayer);\n    musicisplay[p] = 0;\n}}\n\nfunction Resume(tplayer) {{\n    var p = parsePlayer(tplayer);\n    musicisplay[p] = 1;\n}}\n\nfunction CurrentPlayMusic(tplayer) {{\n    var p = parsePlayer(tplayer);\n    return musicnum[p];\n}}\n\nfunction isplaying(tplayer) {{\n    var p = parsePlayer(tplayer);\n    return musicflag[p];\n}}\n"
    ));
    let files = BTreeMap::from([
        ("src/e2/bgm.eps".to_string(), source),
        ("src/e2/tct.py".to_string(), tct_helpers),
    ]);
    E2CompiledSources { files }
}

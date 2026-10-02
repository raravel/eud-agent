use super::{E2SourceKind, ProjectResourceKind};

pub(super) fn packaged_bgm_name(
    source_kind: E2SourceKind,
    kind: ProjectResourceKind,
    need_id: &str,
    original_basename: &str,
) -> Option<String> {
    if source_kind != E2SourceKind::E2p || kind != ProjectResourceKind::Audio {
        return None;
    }
    let index = need_id.strip_prefix("resource:SoundPlayerSET:")?;
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let first_stem = original_basename.split('.').next()?;
    Some(format!("{first_stem}.ogg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_name_is_limited_to_editor2_bgm_context() {
        assert_eq!(
            packaged_bgm_name(
                E2SourceKind::E2p,
                ProjectResourceKind::Audio,
                "resource:SoundPlayerSET:7",
                "raid.live.mp3",
            ),
            Some("raid.ogg".to_string())
        );
        assert_eq!(
            packaged_bgm_name(
                E2SourceKind::E2s,
                ProjectResourceKind::Audio,
                "resource:SoundPlayerSET:7",
                "raid.live.mp3",
            ),
            None
        );
        assert_eq!(
            packaged_bgm_name(
                E2SourceKind::E2p,
                ProjectResourceKind::Audio,
                "resource:FileManagerSET:audio",
                "raid.live.mp3",
            ),
            None
        );
    }
}

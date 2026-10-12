use super::*;

pub(super) fn run() {
    // Given: Editor 2 text retains the stale source path while MoveBGM writes Sound/<first-stem>.ogg.
    let package = FixtureRoot::new("packaged-bgm");
    let source = package.write(
        "producer.e2p",
        b"S_ProjectSET\r\nVersion : 0.17.9.7\r\nE_ProjectSET\r\nS_SoundPlayerSET\r\n1\r\nC:\\lost\\raid.live.mp3\r\nE_SoundPlayerSET\r\n",
    );
    package.write("Map/basemap.scx", b"map");
    package.write("Sound/raid.ogg", b"OggS-packaged-whole-track");
    let mut active = map_only_document(E2SourceKind::E2p, "basemap.scx");
    active.features.push(E2FeatureState {
        feature: E2Feature::SoundBgmReferences,
        activation: E2Activation::Active,
        raw_payload: None,
    });
    active.resources.push(reference(
        "SoundPlayerSET:7",
        E2Feature::SoundBgmReferences,
        ProjectResourceKind::Audio,
        r"C:\lost\raid.live.mp3",
    ));

    // When: the stale original is resolved from the exact whole-track package member.
    let plan = ready(&source, &active, &[], None);

    // Then: the packaged Ogg bytes are owned by the BGM consumer.
    let audio = plan
        .copies
        .iter()
        .find(|copy| copy.resource.id == "SoundPlayerSET:7")
        .expect("active packaged BGM must be copied");
    assert_eq!(audio.bytes, b"OggS-packaged-whole-track");
    assert_eq!(audio.resource.build_destination, "M7_0.ogg");
    assert_eq!(audio.resource.consumer, ProjectResourceConsumer::Mpq);
    assert!(audio
        .resource
        .provenance
        .original_path
        .ends_with("Sound\\raid.ogg"));

    // Given / When: the same reference is inactive.
    let mut inactive = active;
    inactive
        .features
        .iter_mut()
        .find(|state| state.feature == E2Feature::SoundBgmReferences)
        .expect("sound feature state")
        .activation = E2Activation::Inactive;
    let inactive_plan = ready(&source, &inactive, &[], None);

    // Then: inactive provenance remains, with no packaged audio copy or consumer.
    assert_eq!(inactive_plan.copies.len(), 1);
    assert_eq!(inactive_plan.inactive.len(), 1);
    assert_eq!(inactive_plan.inactive[0].id, "SoundPlayerSET:7");
}

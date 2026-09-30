use super::*;

struct TestRoot(PathBuf);

impl Drop for TestRoot {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("tileset regression fixture cleanup failed: {error}");
        }
    }
}

fn opens_generated_map_objects(tileset: u8) {
    // Given a real blank SCX with a start location and an isolated Map session.
    let root = TestRoot(
        std::env::temp_dir().join(format!("map-tileset-regression-{}", uuid::Uuid::new_v4())),
    );
    let dirs = DataDirs::from_bases(&root.0.join("roaming"), &root.0.join("local"));
    dirs.ensure_dirs().unwrap();
    let starcraft = crate::map_context::resolve_starcraft_path(&dirs).unwrap();
    let mut config = dirs.load_config().unwrap();
    config.default_provider = Some(crate::provider::ProviderId::Codex);
    config.providers.codex.default_model = Some("gpt-test".to_string());
    dirs.save_config(&config).unwrap();
    let brush = crate::blank_project::tileset_brushes(&starcraft, tileset)
        .unwrap()
        .into_iter()
        .find(|brush| brush.graphics_valid)
        .expect("tileset has a graphics-valid brush");
    let source = root.0.join("source.scx");
    isom::map_new(
        &source,
        &starcraft,
        &isom::MapNewSpec {
            version: "remastered".to_string(),
            tileset,
            width: 64,
            height: 64,
            terrain_type: brush.id,
            title_bytes_hex: "54696c657365742072656772657373696f6e".to_string(),
            description_bytes_hex: String::new(),
            players: vec![isom::MapNewPlayer {
                slot: 0,
                r#type: "human".to_string(),
                race: "userSelectable".to_string(),
                force: Some(0),
                start: Some(isom::MapNewStart { x: 128, y: 128 }),
            }],
            forces: vec![isom::MapNewForce {
                name_bytes_hex: "5465616d".to_string(),
                allied: false,
                allied_victory: false,
                shared_vision: false,
                random_start: false,
            }],
        },
    )
    .unwrap();
    let context = crate::map_context::MapContextSnapshot {
        revision: crate::map_context::revision_for_path("project".to_string(), &source).unwrap(),
        saved_source_notice: "saved".to_string(),
        source_file_size: std::fs::metadata(&source).unwrap().len(),
        starcraft_path: starcraft,
        digest: crate::chk::digest_chk(&isom::chk_extract(&source).unwrap()),
    };
    let candidates = CandidateStore::new(
        dirs.clone(),
        crate::map_import::MapImportStore::new(dirs.clone()),
    );
    let service = MapAgentService::new(
        dirs.clone(),
        candidates,
        crate::write_coordinator::ProjectWriteCoordinator::silent(),
        crate::project_registry::ProjectRegistry::new(dirs),
    );
    let session = service.create_map_session(&context).unwrap();
    service
        .candidates
        .create_session(&session.meta.id, &context)
        .unwrap();

    // When the Map window loads every object layer through its native service.
    let pages = ["units", "buildings", "doodads", "sprites", "locations"].map(|layer| {
        service.objects(&MapObjectsCommand {
            session_id: session.meta.id.clone(),
            layer: layer.to_string(),
            view: None,
            request_id: None,
            draft_generation: None,
            offset: 0,
            limit: 500,
        })
    });

    // Then every layer loads, including the actual start-location object.
    for page in &pages {
        assert!(page.is_ok(), "tileset {tileset}: {page:?}");
    }
    let units = pages[0].as_ref().unwrap();
    assert_eq!(units["total"], 1);
    assert_eq!(units["items"][0]["object"]["typeId"], 214);
}

macro_rules! tileset_object_regressions {
    ($($name:ident: $tileset:literal),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                opens_generated_map_objects($tileset);
            }
        )+
    };
}

tileset_object_regressions! {
    map_window_objects_load_when_tileset_is_badlands: 0,
    map_window_objects_load_when_tileset_is_platform: 1,
    map_window_objects_load_when_tileset_is_installation: 2,
    map_window_objects_load_when_tileset_is_ashworld: 3,
    map_window_objects_load_when_tileset_is_jungle: 4,
    map_window_objects_load_when_tileset_is_desert: 5,
    map_window_objects_load_when_tileset_is_arctic: 6,
    map_window_objects_load_when_tileset_is_twilight: 7,
}

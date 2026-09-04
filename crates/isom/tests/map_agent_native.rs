use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn fixture() -> PathBuf {
    std::env::var_os("MAP_AGENT_SMOKE_MAP")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("map_agent_rich.scx")
        })
}

fn starcraft_path() -> PathBuf {
    std::env::var_os("STARCRAFT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)\StarCraft"))
}

fn sections(chk: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut output = BTreeMap::<String, Vec<u8>>::new();
    let mut offset = 0_usize;
    while offset + 8 <= chk.len() {
        let name = String::from_utf8_lossy(&chk[offset..offset + 4]).into_owned();
        let size = i32::from_le_bytes(chk[offset + 4..offset + 8].try_into().unwrap());
        if size < 0 {
            break;
        }
        let start = offset + 8;
        let end = start.saturating_add(size as usize).min(chk.len());
        if name == "UNIT" {
            output
                .entry(name)
                .or_default()
                .extend_from_slice(&chk[start..end]);
        } else {
            output.insert(name, chk[start..end].to_vec());
        }
        if end <= offset {
            break;
        }
        offset = end;
    }
    output
}

fn map_header(path: &Path) -> (String, u16, u16, u16) {
    let chk = isom::chk_extract(path).unwrap();
    let sections = sections(&chk);
    let dim = sections.get("DIM ").unwrap();
    let era = sections.get("ERA ").unwrap();
    let mtxm = sections.get("MTXM").unwrap();
    let width = u16::from_le_bytes([dim[0], dim[1]]);
    let height = u16::from_le_bytes([dim[2], dim[3]]);
    let tileset_id = u16::from_le_bytes([era[0], era[1]]) & 7;
    let tileset = [
        "badlands",
        "platform",
        "installation",
        "ashworld",
        "jungle",
        "desert",
        "arctic",
        "twilight",
    ][tileset_id as usize]
        .to_string();
    let tile = u16::from_le_bytes([mtxm[0], mtxm[1]]);
    (tileset, width, height, tile)
}

fn file_hash(path: &Path) -> String {
    let value: Value = serde_json::from_str(&isom::map_digest(path).unwrap()).unwrap();
    value["fileSha256"].as_str().unwrap().to_string()
}

fn temp_map(name: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("eud-map-agent-{name}-{stamp}.scx"))
}
fn decode_bmp_rgb(bmp: &[u8]) -> (usize, usize, Vec<u8>) {
    assert_eq!(&bmp[..2], b"BM");
    let data_offset = u32::from_le_bytes(bmp[10..14].try_into().unwrap()) as usize;
    let width = u32::from_le_bytes(bmp[18..22].try_into().unwrap()) as usize;
    let height = u32::from_le_bytes(bmp[22..26].try_into().unwrap()) as usize;
    assert_eq!(u16::from_le_bytes(bmp[28..30].try_into().unwrap()), 24);
    let row_bytes = (width * 3 + 3) & !3;
    let mut rgb = vec![0_u8; width * height * 3];
    for y in 0..height {
        let source = data_offset + (height - 1 - y) * row_bytes;
        for x in 0..width {
            let input = source + x * 3;
            let output = (y * width + x) * 3;
            rgb[output] = bmp[input + 2];
            rgb[output + 1] = bmp[input + 1];
            rgb[output + 2] = bmp[input];
        }
    }
    (width, height, rgb)
}

fn rgba_to_rgb(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .flat_map(|pixel| pixel[..3].iter().copied())
        .collect()
}
fn fingerprint(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn apply_operations(input: &Path, tag: &str, operations: Value) -> PathBuf {
    let (tileset, width, height, _) = map_header(input);
    let output = temp_map(tag);
    let batch = json!({
        "schema": "eud-map-edit/1",
        "expected": {
            "inputFileSha256": file_hash(input),
            "tileset": tileset,
            "width": width,
            "height": height
        },
        "operations": operations
    });
    isom::mapedit(
        input,
        &output,
        &starcraft_path(),
        batch.to_string().as_bytes(),
    )
    .unwrap();
    output
}

#[test]
fn rich_fixture_has_every_target_and_preservation_authority() {
    let map = fixture();
    let chk = sections(&isom::chk_extract(&map).unwrap());
    assert!(chk.get("MTXM").is_some_and(|bytes| !bytes.is_empty()));
    assert!(chk.get("UNIT").is_some_and(|bytes| {
        bytes
            .chunks_exact(36)
            .any(|unit| u16::from_le_bytes([unit[8], unit[9]]) == 125)
    }));
    assert!(chk.get("DD2 ").is_some_and(|bytes| bytes.len() >= 8));
    assert!(chk.get("THG2").is_some_and(|bytes| bytes.len() >= 10));
    let used_location = chk["MRGN"]
        .chunks_exact(20)
        .enumerate()
        .find(|(index, location)| {
            *index != 63
                && u16::from_le_bytes([location[16], location[17]]) != 0
                && location[..16].iter().any(|byte| *byte != 0)
        })
        .map(|(index, _)| index as u32 + 1)
        .expect("fixture needs a named non-Anywhere location");
    let trigger_uses_location = chk["TRIG"].chunks_exact(2_400).any(|trigger| {
        let condition = (0..16).any(|index| {
            let offset = index * 20;
            u32::from_le_bytes(trigger[offset..offset + 4].try_into().unwrap()) == used_location
        });
        let action = (0..64).any(|index| {
            let offset = 320 + index * 32;
            u32::from_le_bytes(trigger[offset..offset + 4].try_into().unwrap()) == used_location
        });
        condition || action
    });
    assert!(
        trigger_uses_location,
        "fixture location must be trigger-referenced"
    );
    let container: Value = serde_json::from_str(&isom::map_digest(&map).unwrap()).unwrap();
    assert!(container["extraAssets"]["assets"]
        .as_array()
        .is_some_and(|assets| !assets.is_empty()));
}

#[test]
#[ignore = "requires MAP_AGENT_PLATFORM_MAP and installed StarCraft assets"]
fn space_platform_terrain_reveals_star_parallax() {
    let map = PathBuf::from(
        std::env::var_os("MAP_AGENT_PLATFORM_MAP")
            .expect("MAP_AGENT_PLATFORM_MAP must point to a Space Platform map"),
    );
    let (tileset, width, height, _) = map_header(&map);
    assert_eq!(tileset, "platform");

    let chk = sections(&isom::chk_extract(&map).unwrap());
    let terrain = &chk["MTXM"];
    let mut tile_counts = BTreeMap::<u16, usize>::new();
    for tile in terrain.chunks_exact(2) {
        *tile_counts
            .entry(u16::from_le_bytes([tile[0], tile[1]]))
            .or_default() += 1;
    }
    let (&background_tile, &background_count) =
        tile_counts.iter().max_by_key(|(_, count)| *count).unwrap();
    assert!(
        background_count > width as usize * height as usize / 4,
        "platform fixture must use one dominant space tile"
    );

    let scale = 8_usize;
    let request = json!({
        "schema": "eud-map-render/1",
        "mode": "region",
        "x": 0,
        "y": 0,
        "width": width,
        "height": height,
        "scale": scale,
        "layers": ["terrain"]
    });
    let image =
        isom::render_region(&map, &starcraft_path(), request.to_string().as_bytes()).unwrap();
    let pixels_per_tile = 32 / scale;
    let image_width = image.width as usize;
    let star_visible = image
        .rgba
        .chunks_exact(4)
        .enumerate()
        .any(|(index, pixel)| {
            let x = index % image_width;
            let y = index / image_width;
            let tile_x = x / pixels_per_tile;
            let tile_y = y / pixels_per_tile;
            let tile_offset = (tile_y * width as usize + tile_x) * 2;
            let tile = u16::from_le_bytes([terrain[tile_offset], terrain[tile_offset + 1]]);
            tile == background_tile && pixel[3] == 255 && pixel[..3] != [0, 0, 0]
        });
    assert!(
        star_visible,
        "transparent Space Platform terrain must reveal the installed star parallax"
    );

    let crop_width = width.min(16);
    let crop_height = height.min(16);
    let crop_x = (width - crop_width) / 2;
    let crop_y = (height - crop_height) / 2;
    let crop_request = json!({
        "schema": "eud-map-render/1",
        "mode": "region",
        "x": crop_x,
        "y": crop_y,
        "width": crop_width,
        "height": crop_height,
        "scale": scale,
        "layers": ["terrain"]
    });
    let crop =
        isom::render_region(&map, &starcraft_path(), crop_request.to_string().as_bytes()).unwrap();
    let crop_row_bytes = crop.width as usize * 4;
    for row in 0..crop.height as usize {
        let full_start = ((crop_y as usize * pixels_per_tile + row) * image_width
            + crop_x as usize * pixels_per_tile)
            * 4;
        let crop_start = row * crop_row_bytes;
        assert_eq!(
            &crop.rgba[crop_start..crop_start + crop_row_bytes],
            &image.rgba[full_start..full_start + crop_row_bytes],
            "platform star background must remain stable across viewport crops"
        );
    }
}

#[test]
#[ignore = "loads installed StarCraft terrain assets"]
fn terrain_thumbnail_renders_one_exact_tile_and_space_parallax() {
    let starcraft = starcraft_path();
    let catalog = |tileset| {
        let request = json!({
            "schema": "eud-map-catalog/1",
            "kind": "tiles",
            "tileset": tileset,
            "offset": 0,
            "limit": 512,
            "filter": {"graphicsValid": true},
        });
        serde_json::from_str::<Value>(
            &isom::catalog_query(&starcraft, request.to_string().as_bytes()).unwrap(),
        )
        .unwrap()
    };
    let render = |tileset, id| {
        let request = json!({
            "schema": "eud-map-render/1",
            "mode": "thumbnail",
            "layer": "terrain",
            "id": id,
            "owner": 0,
            "tileset": tileset,
        });
        isom::render_region(
            Path::new("missing-palette-thumbnail-source.scx"),
            &starcraft,
            request.to_string().as_bytes(),
        )
        .unwrap()
    };
    let block_is_uniform = |rgba: &[u8], block_x: usize, block_y: usize| {
        let first = ((block_y * 3) * 96 + block_x * 3) * 4;
        (0..3).all(|y| {
            (0..3).all(|x| {
                let offset = ((block_y * 3 + y) * 96 + block_x * 3 + x) * 4;
                rgba[offset..offset + 4] == rgba[first..first + 4]
            })
        })
    };

    let badlands = catalog(0);
    let mut detailed_tiles = 0;
    for entry in badlands["entries"].as_array().unwrap().iter().take(32) {
        let id = entry["id"].as_u64().unwrap();
        let thumbnail = render(0, id);
        assert_eq!((thumbnail.width, thumbnail.height), (96, 96));
        let unique = thumbnail
            .rgba
            .chunks_exact(4)
            .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
            .collect::<BTreeSet<_>>();
        if unique.len() > 1 {
            detailed_tiles += 1;
        }
        for block_y in 0..32 {
            for block_x in 0..32 {
                assert!(
                    block_is_uniform(&thumbnail.rgba, block_x, block_y),
                    "tile {id} must be a single 32x32 tile enlarged exactly 3x"
                );
            }
        }
    }
    assert!(
        detailed_tiles > 0,
        "thumbnail contract must be checked against detailed terrain graphics"
    );

    let platform = catalog(1);
    let reveals_star_parallax = platform["entries"].as_array().unwrap().iter().any(|entry| {
        let thumbnail = render(1, entry["id"].as_u64().unwrap());
        (0..32).any(|block_y| {
            (0..32).any(|block_x| !block_is_uniform(&thumbnail.rgba, block_x, block_y))
        })
    });
    assert!(
        reveals_star_parallax,
        "transparent Space Platform tile thumbnails must reveal star parallax"
    );
}

#[test]
#[ignore = "loads installed StarCraft terrain assets"]
fn catalog_structured_filters_narrow_tiles_before_pagination() {
    let starcraft = starcraft_path();
    let broad_request = json!({
        "schema": "eud-map-catalog/1",
        "kind": "tiles",
        "tileset": 0,
        "offset": 0,
        "limit": 512,
    });
    let broad: Value = serde_json::from_str(
        &isom::catalog_query(&starcraft, broad_request.to_string().as_bytes()).unwrap(),
    )
    .unwrap();
    let exemplar = broad["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["graphicsValid"] == true && entry["walkability"].is_string())
        .expect("installed tileset must expose a graphics-valid tile")
        .clone();
    let terrain_type = exemplar["terrainType"].as_u64().unwrap();
    let group = exemplar["group"].as_u64().unwrap();
    let walkability = exemplar["walkability"].as_str().unwrap();

    let filtered_request = json!({
        "schema": "eud-map-catalog/1",
        "kind": "tiles",
        "tileset": 0,
        "offset": 0,
        "limit": 512,
        "filter": {
            "terrainType": terrain_type,
            "group": group,
            "graphicsValid": true,
            "walkability": walkability,
        },
    });
    let filtered: Value = serde_json::from_str(
        &isom::catalog_query(&starcraft, filtered_request.to_string().as_bytes()).unwrap(),
    )
    .unwrap();
    let entries = filtered["entries"].as_array().unwrap();
    assert!(!entries.is_empty());
    assert_eq!(entries.len() as u64, filtered["total"].as_u64().unwrap());
    assert!(filtered["total"].as_u64().unwrap() < broad["total"].as_u64().unwrap());
    assert!(entries.iter().all(|entry| {
        entry["terrainType"] == terrain_type
            && entry["group"] == group
            && entry["graphicsValid"] == true
            && entry["walkability"] == walkability
    }));

    let incompatible_request = json!({
        "schema": "eud-map-catalog/1",
        "kind": "sprites",
        "tileset": 0,
        "offset": 0,
        "limit": 1,
        "filter": {"terrainType": terrain_type},
    });
    let error =
        isom::catalog_query(&starcraft, incompatible_request.to_string().as_bytes()).unwrap_err();
    assert!(error
        .to_string()
        .contains("catalog filter.terrainType is not supported for sprites"));
}

#[test]
#[ignore = "loads installed StarCraft assets and exercises every mapedit operation family"]
fn every_layer_crud_and_semantic_brush_round_trip() {
    let source = fixture();
    let starcraft = Path::new(r"C:\Program Files (x86)\StarCraft");
    let (tileset, width, height, tile) = map_header(&source);
    let tileset_id = [
        "badlands",
        "platform",
        "installation",
        "ashworld",
        "jungle",
        "desert",
        "arctic",
        "twilight",
    ]
    .iter()
    .position(|candidate| *candidate == tileset)
    .unwrap();
    let brushes: Value = serde_json::from_str(
        &isom::catalog_query(
            starcraft,
            json!({
                "schema": "eud-map-catalog/1",
                "kind": "brushes",
                "tileset": tileset_id,
                "offset": 0,
                "limit": 16
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap(),
    )
    .unwrap();
    let brush = brushes["entries"][0]["id"].as_u64().unwrap();
    let mut generated = Vec::<PathBuf>::new();

    let terrain = apply_operations(
        &source,
        "crud-terrain",
        json!([
            {"op": "terrain.set", "x": 0, "y": 0, "before": tile, "after": tile},
            {"op": "terrain.rect", "x": 1, "y": 1, "width": 2, "height": 2, "after": tile},
            {"op": "terrain.blit", "x": 3, "y": 3, "tiles": [[tile, tile], [tile, tile]]},
            {"op": "terrain.isom_brush", "isomX": 2, "isomY": 2, "brush": brush, "extent": 1}
        ]),
    );
    generated.push(terrain.clone());

    let unit_add = apply_operations(
        &terrain,
        "crud-unit-add",
        json!([{"op": "unit.add", "state": {"typeId": 125, "owner": 4, "x": 512, "y": 512,
            "classId": 123456, "resourceAmount": 77, "stateFlags": 5}}]),
    );
    generated.push(unit_add.clone());
    let unit_bytes = sections(&isom::chk_extract(&unit_add).unwrap())["UNIT"].clone();
    let unit_ordinal = unit_bytes.len() / 36 - 1;
    let unit_before = fingerprint(&unit_bytes[unit_ordinal * 36..unit_ordinal * 36 + 36]);
    let unit_set = apply_operations(
        &unit_add,
        "crud-unit-set",
        json!([{"op": "unit.set", "ordinal": unit_ordinal, "beforeFingerprint": unit_before,
            "state": {"owner": 5, "hpPercent": 75}}]),
    );
    generated.push(unit_set.clone());
    let unit_bytes = sections(&isom::chk_extract(&unit_set).unwrap())["UNIT"].clone();
    let changed_unit = &sections(&isom::chk_extract(&unit_set).unwrap())["UNIT"]
        [unit_ordinal * 36..unit_ordinal * 36 + 36];
    assert_eq!(
        u32::from_le_bytes(changed_unit[0..4].try_into().unwrap()),
        123456
    );
    assert_eq!(
        u16::from_le_bytes(changed_unit[4..6].try_into().unwrap()),
        512
    );
    assert_eq!(
        u32::from_le_bytes(changed_unit[20..24].try_into().unwrap()),
        77
    );
    assert_eq!(
        u16::from_le_bytes(changed_unit[26..28].try_into().unwrap()),
        5
    );
    let unit_before = fingerprint(&unit_bytes[unit_ordinal * 36..unit_ordinal * 36 + 36]);
    let unit_move = apply_operations(
        &unit_set,
        "crud-unit-move",
        json!([{"op": "unit.move", "ordinal": unit_ordinal, "beforeFingerprint": unit_before, "x": 576, "y": 544}]),
    );
    generated.push(unit_move.clone());
    let unit_bytes = sections(&isom::chk_extract(&unit_move).unwrap())["UNIT"].clone();
    let unit_before = fingerprint(&unit_bytes[unit_ordinal * 36..unit_ordinal * 36 + 36]);
    let unit_delete = apply_operations(
        &unit_move,
        "crud-unit-delete",
        json!([{"op": "unit.delete", "ordinal": unit_ordinal, "beforeFingerprint": unit_before}]),
    );
    generated.push(unit_delete.clone());
    assert_eq!(
        sections(&isom::chk_extract(&unit_delete).unwrap())["UNIT"].len(),
        sections(&isom::chk_extract(&terrain).unwrap())["UNIT"].len()
    );

    let doodads: Value = serde_json::from_str(
        &isom::catalog_query(
            starcraft,
            json!({
                "schema": "eud-map-catalog/1",
                "kind": "doodads",
                "tileset": tileset_id,
                "offset": 0,
                "limit": 512
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap(),
    )
    .unwrap();
    let doodad_entry = doodads["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["graphicsValid"] == true)
        .unwrap();
    let doodad_id = doodad_entry["id"].as_u64().unwrap();
    let doodad_width = doodad_entry["width"].as_u64().unwrap() as usize;
    let doodad_height = doodad_entry["height"].as_u64().unwrap() as usize;
    let replacement_tiles = vec![vec![Value::from(tile); doodad_width]; doodad_height];
    let doodad_add = apply_operations(
        &unit_delete,
        "crud-doodad-add",
        json!([{"op": "doodad.add", "state": {"doodadId": doodad_id, "x": 640, "y": 640, "owner": 11}}]),
    );
    generated.push(doodad_add.clone());
    let doodad_bytes = sections(&isom::chk_extract(&doodad_add).unwrap())["DD2 "].clone();
    let doodad_ordinal = doodad_bytes.len() / 8 - 1;
    let doodad_before = fingerprint(&doodad_bytes[doodad_ordinal * 8..doodad_ordinal * 8 + 8]);
    let doodad_set = apply_operations(
        &doodad_add,
        "crud-doodad-set",
        json!([{"op": "doodad.set", "ordinal": doodad_ordinal, "beforeFingerprint": doodad_before,
            "replacementTiles": replacement_tiles.clone(), "state": {"doodadId": doodad_id, "x": 672, "y": 640, "owner": 10, "disabled": false}}]),
    );
    generated.push(doodad_set.clone());
    let doodad_bytes = sections(&isom::chk_extract(&doodad_set).unwrap())["DD2 "].clone();
    let doodad_before = fingerprint(&doodad_bytes[doodad_ordinal * 8..doodad_ordinal * 8 + 8]);
    let doodad_move = apply_operations(
        &doodad_set,
        "crud-doodad-move",
        json!([{"op": "doodad.move", "ordinal": doodad_ordinal, "beforeFingerprint": doodad_before,
            "replacementTiles": replacement_tiles.clone(), "x": 704, "y": 672}]),
    );
    generated.push(doodad_move.clone());
    let doodad_bytes = sections(&isom::chk_extract(&doodad_move).unwrap())["DD2 "].clone();
    let doodad_before = fingerprint(&doodad_bytes[doodad_ordinal * 8..doodad_ordinal * 8 + 8]);
    let doodad_delete = apply_operations(
        &doodad_move,
        "crud-doodad-delete",
        json!([{"op": "doodad.delete", "ordinal": doodad_ordinal, "beforeFingerprint": doodad_before,
            "replacementTiles": replacement_tiles}]),
    );
    generated.push(doodad_delete.clone());

    let sprite_add = apply_operations(
        &doodad_delete,
        "crud-sprite-add",
        json!([{"op": "sprite.add", "state": {"spriteId": 301, "x": 768, "y": 704, "owner": 3, "flags": 4096}}]),
    );
    generated.push(sprite_add.clone());
    let sprite_bytes = sections(&isom::chk_extract(&sprite_add).unwrap())["THG2"].clone();
    let sprite_ordinal = sprite_bytes.len() / 10 - 1;
    let sprite_before = fingerprint(&sprite_bytes[sprite_ordinal * 10..sprite_ordinal * 10 + 10]);
    let sprite_set = apply_operations(
        &sprite_add,
        "crud-sprite-set",
        json!([{"op": "sprite.set", "ordinal": sprite_ordinal, "beforeFingerprint": sprite_before,
            "state": {"spriteId": 301, "x": 800, "y": 704, "owner": 4, "flags": 4096}}]),
    );
    generated.push(sprite_set.clone());
    let sprite_bytes = sections(&isom::chk_extract(&sprite_set).unwrap())["THG2"].clone();
    let sprite_before = fingerprint(&sprite_bytes[sprite_ordinal * 10..sprite_ordinal * 10 + 10]);
    let sprite_move = apply_operations(
        &sprite_set,
        "crud-sprite-move",
        json!([{"op": "sprite.move", "ordinal": sprite_ordinal, "beforeFingerprint": sprite_before, "x": 832, "y": 736}]),
    );
    generated.push(sprite_move.clone());
    let sprite_bytes = sections(&isom::chk_extract(&sprite_move).unwrap())["THG2"].clone();
    let sprite_before = fingerprint(&sprite_bytes[sprite_ordinal * 10..sprite_ordinal * 10 + 10]);
    let sprite_delete = apply_operations(
        &sprite_move,
        "crud-sprite-delete",
        json!([{"op": "sprite.delete", "ordinal": sprite_ordinal, "beforeFingerprint": sprite_before}]),
    );
    generated.push(sprite_delete.clone());

    let location_before = sections(&isom::chk_extract(&sprite_delete).unwrap())["MRGN"].clone();
    let location_add = apply_operations(
        &sprite_delete,
        "crud-location-add",
        json!([{"op": "location.add", "state": {"locationId": 0, "left": 320, "top": 320,
            "right": 448, "bottom": 448, "nameBytesHex": "43727564204c6f636174696f6e"}}]),
    );
    generated.push(location_add.clone());
    let location_after = sections(&isom::chk_extract(&location_add).unwrap())["MRGN"].clone();
    let location_id = location_before
        .chunks_exact(20)
        .zip(location_after.chunks_exact(20))
        .position(|(before, after)| before != after)
        .map(|index| index + 1)
        .expect("location.add must occupy one stable slot");
    let location_set = apply_operations(
        &location_add,
        "crud-location-set",
        json!([{"op": "location.set", "state": {"locationId": location_id, "left": 352, "top": 352,
            "right": 480, "bottom": 480, "elevationFlags": 3}}]),
    );
    let set_location = &sections(&isom::chk_extract(&location_set).unwrap())["MRGN"]
        [(location_id - 1) * 20..location_id * 20];
    assert_eq!(
        i32::from_le_bytes(set_location[0..4].try_into().unwrap()),
        352
    );
    assert_eq!(
        i32::from_le_bytes(set_location[4..8].try_into().unwrap()),
        352
    );
    assert_eq!(
        i32::from_le_bytes(set_location[8..12].try_into().unwrap()),
        480
    );
    assert_eq!(
        i32::from_le_bytes(set_location[12..16].try_into().unwrap()),
        480
    );
    assert_ne!(
        u16::from_le_bytes(set_location[16..18].try_into().unwrap()),
        0
    );
    generated.push(location_set.clone());
    let location_rename = apply_operations(
        &location_set,
        "crud-location-rename",
        json!([{"op": "location.rename", "locationId": location_id, "nameBytesHex": "52656e616d65642043727564"}]),
    );
    generated.push(location_rename.clone());
    let location_delete = apply_operations(
        &location_rename,
        "crud-location-delete",
        json!([{"op": "location.delete", "locationId": location_id}]),
    );
    generated.push(location_delete.clone());
    let deleted_location = &sections(&isom::chk_extract(&location_delete).unwrap())["MRGN"]
        [(location_id - 1) * 20..location_id * 20];
    assert!(deleted_location.iter().all(|byte| *byte == 0));

    let anywhere_output = temp_map("crud-anywhere-invalid");
    let anywhere_batch = json!({
        "schema": "eud-map-edit/1",
        "expected": {
            "inputFileSha256": file_hash(&location_delete),
            "tileset": tileset,
            "width": width,
            "height": height
        },
        "operations": [{"op": "location.delete", "locationId": 64}]
    });
    assert!(isom::mapedit(
        &location_delete,
        &anywhere_output,
        &starcraft_path(),
        anywhere_batch.to_string().as_bytes()
    )
    .is_err());
    assert!(!anywhere_output.exists());

    let original = sections(&isom::chk_extract(&source).unwrap());
    let final_sections = sections(&isom::chk_extract(&location_delete).unwrap());
    let mutable = [
        "MTXM", "TILE", "ISOM", "UNIT", "DD2 ", "THG2", "MRGN", "STR ", "STRx",
    ];
    for (name, bytes) in &original {
        if !mutable.contains(&name.as_str()) {
            assert_eq!(
                final_sections.get(name),
                Some(bytes),
                "unsupported section {name}"
            );
        }
    }
    let original_container: Value =
        serde_json::from_str(&isom::map_digest(&source).unwrap()).unwrap();
    let final_container: Value =
        serde_json::from_str(&isom::map_digest(&location_delete).unwrap()).unwrap();
    assert_eq!(
        original_container["extraAssets"]["digest"],
        final_container["extraAssets"]["digest"]
    );
    for path in generated {
        fs::remove_file(path).ok();
    }
}

#[test]
#[ignore = "loads installed StarCraft DAT/GRP assets and exercises native map editing"]
fn mixed_batch_is_atomic_preserves_container_and_renders_real_assets() {
    let source = fixture();
    let (tileset, width, height, tile) = map_header(&source);
    assert!(width >= 8 && height >= 8);
    let starcraft = Path::new(r"C:\Program Files (x86)\StarCraft");
    let tileset_id = [
        "badlands",
        "platform",
        "installation",
        "ashworld",
        "jungle",
        "desert",
        "arctic",
        "twilight",
    ]
    .iter()
    .position(|candidate| *candidate == tileset)
    .unwrap();
    let before_container: Value =
        serde_json::from_str(&isom::map_digest(&source).unwrap()).unwrap();
    assert!(
        before_container["extraAssets"]["assets"]
            .as_array()
            .is_some_and(|assets| !assets.is_empty()),
        "rich native fixture must contain at least one extra MPQ asset"
    );
    let invalid_output = temp_map("invalid");
    let invalid = json!({
        "schema": "eud-map-edit/1",
        "expected": {
            "inputFileSha256": file_hash(&source),
            "tileset": tileset,
            "width": width,
            "height": height
        },
        "operations": [
            {"op": "terrain.rect", "x": 0, "y": 0, "width": 2, "height": 2, "after": tile},
            {"op": "unit.add", "state": {"typeId": 999, "owner": 0, "x": 96, "y": 96}}
        ]
    });
    let error = isom::mapedit(
        &source,
        &invalid_output,
        &starcraft_path(),
        invalid.to_string().as_bytes(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("unit type is out of range"),
        "unexpected native error: {error}"
    );
    assert!(
        !invalid_output.exists(),
        "invalid mixed batch must not create output"
    );
    let doodad_query = json!({
        "schema": "eud-map-catalog/1",
        "kind": "doodads",
        "tileset": tileset_id,
        "offset": 0,
        "limit": 512
    });
    let doodads: Value = serde_json::from_str(
        &isom::catalog_query(starcraft, doodad_query.to_string().as_bytes()).unwrap(),
    )
    .unwrap();
    let doodad_entries = doodads["entries"].as_array().unwrap();
    let doodad_entry = doodad_entries
        .iter()
        .find(|entry| entry["graphicsValid"] == true && entry["overlay"] == true)
        .or_else(|| {
            doodad_entries
                .iter()
                .find(|entry| entry["graphicsValid"] == true)
        })
        .unwrap_or_else(|| panic!("tileset must expose a graphics-valid doodad: {doodads}"));
    let doodad_id = doodad_entry["id"].as_u64().unwrap();
    let doodad_has_overlay = doodad_entry["overlay"] == true;

    let retained_output = std::env::var_os("MAP_AGENT_FIXTURE_OUT").map(PathBuf::from);
    let output = retained_output.clone().unwrap_or_else(|| temp_map("valid"));
    let valid = json!({
        "schema": "eud-map-edit/1",
        "expected": {
            "inputFileSha256": file_hash(&source),
            "tileset": tileset,
            "width": width,
            "height": height
        },
        "operations": [
            {"op": "terrain.rect", "x": 0, "y": 0, "width": 2, "height": 2, "after": tile},
            {"op": "unit.add", "state": {"typeId": 125, "owner": 4, "x": 160, "y": 160}},
            {"op": "doodad.add", "state": {"doodadId": doodad_id, "x": 256, "y": 256, "owner": 11}},
            {"op": "sprite.add", "state": {"spriteId": 301, "x": 320, "y": 256, "owner": 5, "flags": 4096}},
            {"op": "location.add", "state": {
                "locationId": 0,
                "left": 128,
                "top": 128,
                "right": 256,
                "bottom": 256,
                "nameBytesHex": "4d6170204167656e7420536d6f6b65"
            }}
        ]
    });
    let report: Value = serde_json::from_str(
        &isom::mapedit(
            &source,
            &output,
            &starcraft_path(),
            valid.to_string().as_bytes(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(report["operationCount"], 5);
    assert!(output.is_file());

    let before_chk = sections(&isom::chk_extract(&source).unwrap());
    let after_chk = sections(&isom::chk_extract(&output).unwrap());
    assert_eq!(after_chk["UNIT"].len(), before_chk["UNIT"].len() + 36);
    assert_eq!(
        after_chk["DD2 "].len(),
        before_chk.get("DD2 ").map_or(0, Vec::len) + 8
    );
    assert_eq!(
        after_chk["THG2"].len(),
        before_chk.get("THG2").map_or(0, Vec::len)
            + 10 * (1 + if doodad_has_overlay { 1 } else { 0 })
    );
    assert_eq!(
        after_chk["TRIG"], before_chk["TRIG"],
        "trigger bytes must remain exact"
    );
    let after_container: Value = serde_json::from_str(&isom::map_digest(&output).unwrap()).unwrap();
    assert_eq!(
        before_container["extraAssets"]["digest"], after_container["extraAssets"]["digest"],
        "extra MPQ assets must remain byte-identical"
    );

    let render_request = json!({
        "schema": "eud-map-render/1",
        "mode": "region",
        "x": 0,
        "y": 0,
        "width": 16.min(width),
        "height": 16.min(height),
        "scale": 2,
        "layers": ["terrain", "doodads", "sprites", "units", "buildings", "locations"]
    });
    let image =
        isom::render_region(&output, starcraft, render_request.to_string().as_bytes()).unwrap();
    assert_eq!(
        image.rgba.len(),
        image.width as usize * image.height as usize * 4
    );
    assert!(image.rgba.chunks_exact(4).any(|pixel| pixel[3] == 255));
    let mut invalid_scale_request = render_request.clone();
    invalid_scale_request["scale"] = json!(3);
    let error = isom::render_region(
        &output,
        starcraft,
        invalid_scale_request.to_string().as_bytes(),
    )
    .expect_err("unsupported render scale must return an actionable error");
    assert_eq!(
        error.detail.as_deref(),
        Some("render scale must be 1, 2, 4, or 8")
    );

    let full_request = json!({
        "schema": "eud-map-render/1",
        "mode": "region",
        "x": 0,
        "y": 0,
        "width": width,
        "height": height,
        "scale": 8,
        "layers": ["terrain"]
    });
    let full =
        isom::render_region(&output, starcraft, full_request.to_string().as_bytes()).unwrap();
    let legacy_bmp = isom::render_map(&output, starcraft, 8).unwrap();
    let (legacy_width, legacy_height, legacy_rgb) = decode_bmp_rgb(&legacy_bmp);
    assert_eq!(
        (full.width as usize, full.height as usize),
        (legacy_width, legacy_height)
    );
    assert_eq!(
        rgba_to_rgb(&full.rgba),
        legacy_rgb,
        "terrain pixels must match verified renderer"
    );

    let crop_request = json!({
        "schema": "eud-map-render/1",
        "mode": "region",
        "x": 2,
        "y": 2,
        "width": 4,
        "height": 4,
        "scale": 8,
        "layers": ["terrain"]
    });
    let crop =
        isom::render_region(&output, starcraft, crop_request.to_string().as_bytes()).unwrap();
    let tile_pixels = 4_usize;
    for row in 0..crop.height as usize {
        let full_start = ((2 * tile_pixels + row) * full.width as usize + 2 * tile_pixels) * 4;
        let crop_start = row * crop.width as usize * 4;
        assert_eq!(
            &crop.rgba[crop_start..crop_start + crop.width as usize * 4],
            &full.rgba[full_start..full_start + crop.width as usize * 4]
        );
    }

    for kind in [
        "tiles",
        "brushes",
        "units",
        "buildings",
        "doodads",
        "sprites",
    ] {
        let request = json!({
            "schema": "eud-map-catalog/1",
            "kind": kind,
            "tileset": before_container["tileset"]
                .as_str()
                .and_then(|name| ["badlands", "platform", "installation", "ashworld", "jungle", "desert", "arctic", "twilight"].iter().position(|candidate| candidate == &name))
                .unwrap(),
            "offset": 0,
            "limit": 4
        });
        let catalog: Value = serde_json::from_str(
            &isom::catalog_query(starcraft, request.to_string().as_bytes()).unwrap(),
        )
        .unwrap();
        assert!(
            catalog["entries"]
                .as_array()
                .is_some_and(|entries| !entries.is_empty()),
            "{kind}"
        );
    }

    for (layer, id, owner) in [
        ("terrain", u64::from(tile), 0_u8),
        ("units", 0, 0),
        ("buildings", 125, 4),
        ("doodads", doodad_id, 11),
        ("sprites", 301, 5),
    ] {
        let thumbnail = json!({
            "schema": "eud-map-render/1",
            "mode": "thumbnail",
            "layer": layer,
            "id": id,
            "owner": owner,
            "tileset": tileset_id
        });
        let first =
            isom::render_region(&output, starcraft, thumbnail.to_string().as_bytes()).unwrap();
        let second =
            isom::render_region(&output, starcraft, thumbnail.to_string().as_bytes()).unwrap();
        assert_eq!((first.width, first.height), (96, 96));
        assert_eq!(first, second, "{layer} thumbnail must be deterministic");
        assert!(
            first
                .rgba
                .chunks_exact(4)
                .any(|pixel| pixel[0] != 17 || pixel[1] != 24 || pixel[2] != 39),
            "{layer} thumbnail must contain actual asset pixels"
        );
    }
    let map_independent_thumbnail = json!({
        "schema": "eud-map-render/1",
        "mode": "thumbnail",
        "layer": "terrain",
        "id": u64::from(tile),
        "owner": 0,
        "tileset": tileset_id
    });
    let missing_map = output.with_extension("missing-thumbnail-source.scx");
    let thumbnail = isom::render_region(
        &missing_map,
        starcraft,
        map_independent_thumbnail.to_string().as_bytes(),
    )
    .expect("palette thumbnails must not reopen or parse the candidate map");
    assert_eq!((thumbnail.width, thumbnail.height), (96, 96));

    let player_thumbnail = |owner| {
        json!({
            "schema": "eud-map-render/1",
            "mode": "thumbnail",
            "layer": "buildings",
            "id": 125,
            "owner": owner,
            "tileset": tileset_id
        })
    };
    let player_one = isom::render_region(
        &output,
        starcraft,
        player_thumbnail(0).to_string().as_bytes(),
    )
    .unwrap();
    let player_two = isom::render_region(
        &output,
        starcraft,
        player_thumbnail(1).to_string().as_bytes(),
    )
    .unwrap();
    assert_ne!(
        player_one.rgba, player_two.rgba,
        "player remap colors must differ"
    );

    if retained_output.is_none() {
        fs::remove_file(output).ok();
    }
}

#[test]
#[ignore = "loads installed StarCraft CV5/VX4/VR4/WPE assets for all eight tilesets"]
fn image_quantizer_uses_only_stable_graphics_valid_tiles_for_every_tileset() {
    let starcraft = starcraft_path();
    for tileset in 0_u16..8 {
        let catalog: Value = serde_json::from_str(
            &isom::catalog_query(
                &starcraft,
                json!({
                    "schema": "eud-map-catalog/1",
                    "kind": "tiles",
                    "tileset": tileset,
                    "offset": 0,
                    "limit": 512,
                    "query": "",
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap(),
        )
        .unwrap();
        let before_tile = catalog["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["graphicsValid"] == true)
            .and_then(|entry| entry["id"].as_u64())
            .unwrap() as u16;
        let mut rgba = Vec::with_capacity(8 * 8 * 4);
        for y in 0_u8..8 {
            for x in 0_u8..8 {
                rgba.extend_from_slice(&[
                    x.saturating_mul(31),
                    y.saturating_mul(29),
                    x.wrapping_add(y).saturating_mul(15),
                    if x == 0 && y == 0 {
                        0
                    } else if x == 1 {
                        128
                    } else {
                        255
                    },
                ]);
            }
        }
        let before = vec![before_tile; 64];
        let first = isom::image_quantize(&starcraft, tileset, &rgba, 8, 8, &before).unwrap();
        let second = isom::image_quantize(&starcraft, tileset, &rgba, 8, 8, &before).unwrap();
        assert_eq!(first, second, "tileset {tileset}");
        assert_eq!(first.tiles[0], before_tile, "transparent cell {tileset}");
        assert_eq!(first.preview_rgb.len(), 8 * 8 * 3);
        assert!(first.unique_tile_count > 0);
        for tile in std::collections::BTreeSet::from_iter(first.tiles.iter().copied()) {
            let entry: Value = serde_json::from_str(
                &isom::catalog_query(
                    &starcraft,
                    json!({
                        "schema": "eud-map-catalog/1",
                        "kind": "tiles",
                        "tileset": tileset,
                        "offset": tile,
                        "limit": 1,
                        "query": "",
                    })
                    .to_string()
                    .as_bytes(),
                )
                .unwrap(),
            )
            .unwrap();
            let entry = &entry["entries"][0];
            assert_eq!(entry["id"], tile, "tileset {tileset}");
            assert_eq!(
                entry["graphicsValid"], true,
                "tileset {tileset} tile {tile}"
            );
        }
    }
}

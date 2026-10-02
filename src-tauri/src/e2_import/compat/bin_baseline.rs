use super::bin::{delta, invalid, operation, pack, selected, validate_pointer, EPD_BASE};
use super::{E2BinBaseline, E2BinRect, E2CompatRequest};
use crate::e2_import::E2RawValue;

pub(super) const SOURCE_COMMIT: &str = "2baf971ee493c772c609a938e0daffba3c40792c";
pub(super) const CANONICAL_DIGEST: &str =
    "869404e2ad09335029882be734e40ed8bff03feed31505b3e98166e76d600ce3";

pub(super) fn v0_17_9_7() -> Vec<E2BinBaseline> {
    (0u8..26)
        .map(|row| {
            let index = usize::from(row);
            let values = RECTS[index];
            let start = STARTS[index];
            E2BinBaseline {
                row,
                name: NAMES[index],
                pointer: POINTERS[index],
                rect: E2BinRect {
                    x: values[0],
                    y: values[1],
                    x2: values[2],
                    y2: values[3],
                    width: values[4],
                    height: values[5],
                },
                objects: super::bin_objects::OBJECTS[start..start + COUNTS[index]].to_vec(),
            }
        })
        .collect()
}

const NAMES: [&str; 26] = [
    "statres",
    "statdata",
    "statport",
    "stat_f10",
    "minimap",
    "ptextbox",
    "statbtnp",
    "statflufp1",
    "statflufp2",
    "statflufp3",
    "statflufp4",
    "ttextbox",
    "statbtnt",
    "statfluft1",
    "statfluft2",
    "statfluft3",
    "statfluft4",
    "statfluft5",
    "ztextbox",
    "statbtnz",
    "statflufz1",
    "statflufz2",
    "statflufz3",
    "statflufz4",
    "statflufz5",
    "statflufz6",
];
const POINTERS: [u32; 26] = [
    0x68C234, 0x68C1F0, 0x68AC8C, 0x68C224, 0x59CB5C, 0x68C140, 0x68C148, 0x515350, 0x51535C,
    0x515368, 0x515374, 0x68C140, 0x68C148, 0x515308, 0x515314, 0x515320, 0x51532C, 0x515338,
    0x68C148, 0x68C148, 0x5152B0, 0x5152BC, 0x5152C8, 0x5152D4, 0x5152E0, 0x5152EC,
];
const RECTS: [[i16; 6]; 26] = [
    [220, 0, 419, 19, 420, 20],
    [138, 388, 269, 91, 270, 92],
    [408, 408, 87, 71, 88, 72],
    [408, 388, 87, 19, 88, 20],
    [0, 315, 137, 164, 138, 165],
    [190, 338, 257, 17, 258, 18],
    [496, 354, 143, 125, 144, 126],
    [0, 302, 147, 314, 148, 13],
    [138, 315, 184, 353, 47, 39],
    [138, 354, 495, 387, 358, 34],
    [453, 330, 639, 353, 187, 24],
    [170, 346, 279, 17, 280, 18],
    [496, 354, 143, 125, 144, 126],
    [0, 293, 22, 314, 23, 22],
    [138, 315, 165, 366, 28, 52],
    [138, 367, 495, 387, 358, 21],
    [444, 354, 495, 366, 52, 13],
    [466, 325, 639, 353, 174, 29],
    [190, 338, 269, 17, 270, 18],
    [496, 354, 143, 125, 144, 126],
    [0, 289, 28, 314, 29, 26],
    [29, 306, 114, 314, 86, 9],
    [138, 315, 165, 387, 28, 73],
    [166, 345, 443, 387, 278, 43],
    [444, 354, 495, 387, 52, 34],
    [466, 312, 639, 353, 174, 42],
];
const STARTS: [usize; 26] = [
    0, 8, 65, 67, 68, 72, 79, 88, 89, 90, 91, 92, 99, 108, 109, 110, 111, 112, 113, 120, 129, 130,
    131, 132, 133, 134,
];
const COUNTS: [usize; 26] = [
    8, 57, 2, 1, 4, 7, 9, 1, 1, 1, 1, 7, 9, 1, 1, 1, 1, 1, 7, 9, 1, 1, 1, 1, 1, 1,
];
pub(super) fn render_scenes(
    request: &E2CompatRequest<'_>,
    section: &crate::e2_import::E2RawSection,
    output: &mut String,
) -> Result<(), crate::e2_import::E2ImportIssue> {
    let race = request.player_race.unwrap_or(255);
    if race == 255 {
        return Ok(());
    }
    if race > 2 {
        return Err(invalid("BIN PlayerRace must be 0, 1, 2, or 255"));
    }
    let baselines = v0_17_9_7();
    for baseline in baselines.iter().filter(|row| selected(race, row.row)) {
        validate_pointer(baseline.pointer)?;
        let prefix = format!(".(0)({})", baseline.row);
        if bool_field(section, &format!("{prefix}isDefault"))? {
            let rect = changed_rect(section, &prefix, baseline.rect)?;
            render_move(baseline.pointer, 0, rect, baseline.rect, output);
        }
        for (index, original) in baseline.objects.iter().copied().enumerate() {
            let object_prefix = format!(".(0)({})({index})", baseline.row);
            if bool_field(section, &format!("{object_prefix}isDefault"))? {
                let changed = changed_rect(section, &object_prefix, original.rect)?;
                let object =
                    u8::try_from(index + 1).map_err(|_| invalid("BIN object index exceeds 255"))?;
                render_move(baseline.pointer, object, changed, original.rect, output);
            }
        }
    }
    Ok(())
}

fn render_move(
    pointer: u32,
    object: u8,
    changed: super::E2BinRect,
    old: super::E2BinRect,
    output: &mut String,
) {
    let epd = (i64::from(pointer) - i64::from(EPD_BASE)) / 4;
    let object_offset = u32::from(object) * 86 / 4;
    if object == 0 {
        let right = changed.x.saturating_add(changed.width).saturating_sub(1);
        let bottom = changed.y.saturating_add(changed.height).saturating_sub(1);
        output.push_str(&format!(
            "f_dwwrite_epd(f_epdread_epd({epd}) + 1, {});\n",
            pack(changed.x, changed.y)
        ));
        output.push_str(&format!(
            "f_dwwrite_epd(f_epdread_epd({epd}) + 2, {});\n",
            pack(right, bottom)
        ));
        output.push_str(&format!(
            "f_dwwrite_epd(f_epdread_epd({epd}) + 3, {});\n",
            pack(changed.width, changed.height)
        ));
        output.push_str(&format!(
            "SetDeaths(f_epdread_epd({epd}) + 13, {}, {}, 0);\n",
            operation(changed.width, old.width),
            delta(changed.width, old.width) * 65536
        ));
        output.push_str(&format!(
            "SetDeaths(f_epdread_epd({epd}) + 14, {}, {}, 0);\n",
            operation(changed.height, old.height),
            delta(changed.height, old.height)
        ));
    } else if object % 2 == 0 {
        let right = changed.x.saturating_add(changed.width).saturating_sub(1);
        let bottom = changed.y.saturating_add(changed.height).saturating_sub(1);
        output.push_str(&format!(
            "f_dwwrite_epd(f_epdread_epd({epd}) + {}, {});\n",
            1 + object_offset,
            pack(changed.x, changed.y)
        ));
        output.push_str(&format!(
            "f_dwwrite_epd(f_epdread_epd({epd}) + {}, {});\n",
            2 + object_offset,
            pack(right, bottom)
        ));
    } else {
        output.push_str(&format!(
            "SetDeaths(f_epdread_epd({epd}) + {}, {}, {}, 0);\n",
            1 + object_offset,
            operation(changed.x, old.x),
            delta(changed.x, old.x) * 65536
        ));
        let right = changed.x.saturating_add(changed.width).saturating_sub(1);
        output.push_str(&format!(
            "SetDeaths(f_epdread_epd({epd}) + {}, SetTo, {}, 0);\n",
            2 + object_offset,
            pack(changed.y, right)
        ));
        output.push_str(&format!(
            "SetDeaths(f_epdread_epd({epd}) + {}, {}, {}, 0);\n",
            3 + object_offset,
            operation(changed.y, old.y),
            delta(changed.y, old.y)
        ));
    }
}

fn changed_rect(
    section: &crate::e2_import::E2RawSection,
    prefix: &str,
    fallback: super::E2BinRect,
) -> Result<super::E2BinRect, crate::e2_import::E2ImportIssue> {
    Ok(super::E2BinRect {
        x: integer(section, &format!("{prefix}posx"), fallback.x)?,
        y: integer(section, &format!("{prefix}posy"), fallback.y)?,
        x2: fallback.x2,
        y2: fallback.y2,
        width: integer(section, &format!("{prefix}sizew"), fallback.width)?,
        height: integer(section, &format!("{prefix}sizeh"), fallback.height)?,
    })
}

fn integer(
    section: &crate::e2_import::E2RawSection,
    key: &str,
    fallback: i16,
) -> Result<i16, crate::e2_import::E2ImportIssue> {
    match section.fields.get(key) {
        None => Ok(fallback),
        Some(E2RawValue::Integer(value)) => {
            i16::try_from(*value).map_err(|_| invalid("BIN coordinate is outside i16"))
        }
        Some(_) => Err(invalid("BIN coordinate must be an integer")),
    }
}

fn bool_field(
    section: &crate::e2_import::E2RawSection,
    key: &str,
) -> Result<bool, crate::e2_import::E2ImportIssue> {
    match section.fields.get(key) {
        None => Ok(false),
        Some(E2RawValue::Bool(value)) => Ok(*value),
        Some(_) => Err(invalid("BIN change flag must be boolean")),
    }
}

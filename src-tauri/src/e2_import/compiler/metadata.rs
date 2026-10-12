use std::collections::BTreeMap;

use super::super::{E2Activation, E2ImportIr, E2RawValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MemoryField {
    pub(super) address: i64,
    pub(super) width: u8,
}

#[derive(Default)]
pub(super) struct CompileMetadata {
    buttons: BTreeMap<u32, Vec<Button>>,
    _tbl_overrides: BTreeMap<u32, String>,
}

struct Button {
    position: u32,
    enabled_string: u32,
    disabled_string: u32,
}

impl CompileMetadata {
    pub(super) fn from_document(document: &E2ImportIr) -> Result<Self, String> {
        let mut metadata = Self::default();
        for section in &document.raw_sections {
            if section.activation != E2Activation::Active {
                continue;
            }
            match section.name.as_str() {
                "BtnSET" => metadata.decode_buttons(&section.fields)?,
                "FileManagerSET" => metadata.decode_tbl_overrides(&section.fields)?,
                _ => {}
            }
        }
        Ok(metadata)
    }

    pub(super) fn dat_field(&self, table: usize, field: usize) -> Option<MemoryField> {
        let facts = DAT_FIELDS.get(table)?.get(field)?;
        Some(MemoryField {
            address: DAT_BASES[table] + i64::from(unpack_offset(*facts)),
            width: unpack_width(*facts),
        })
    }

    pub(super) fn cunit_field(&self, field: usize) -> Option<MemoryField> {
        let facts = CUNIT_FIELDS.get(field)?;
        Some(MemoryField {
            address: i64::from(unpack_offset(*facts)),
            width: unpack_width(*facts),
        })
    }

    pub(super) fn button_position(&self, selection: &str) -> Option<(u32, usize)> {
        let (unit, index) = selection.split_once(':')?;
        let unit = unit.parse::<u32>().ok()?;
        let index = index.parse::<usize>().ok()?;
        let mut order = self
            .buttons
            .get(&unit)?
            .iter()
            .enumerate()
            .collect::<Vec<_>>();
        order.sort_by_key(|(_, button)| button.position);
        let position = order.iter().position(|(original, _)| *original == index)?;
        Some((unit, position))
    }

    pub(super) fn button_string(&self, selection: &str, enabled: bool) -> Option<u32> {
        let (unit, index) = selection.split_once(':')?;
        let button = self
            .buttons
            .get(&unit.parse::<u32>().ok()?)?
            .get(index.parse::<usize>().ok()?)?;
        Some(if enabled {
            button.enabled_string
        } else {
            button.disabled_string
        })
    }

    fn decode_buttons(&mut self, fields: &BTreeMap<String, E2RawValue>) -> Result<(), String> {
        for (key, value) in fields {
            let Some(unit_text) = key.strip_prefix("BtnUse") else {
                continue;
            };
            if !raw_bool(value)? {
                continue;
            }
            let unit = unit_text
                .parse::<u32>()
                .map_err(|_| format!("invalid {key}"))?;
            let data_key = format!("BtnData{unit}");
            let csv = raw_string(
                fields
                    .get(&data_key)
                    .ok_or_else(|| format!("missing {data_key}"))?,
            )?;
            let csv = csv.strip_suffix(',').unwrap_or(csv);
            let values = csv
                .split(',')
                .map(|value| {
                    value
                        .parse::<u32>()
                        .map_err(|_| format!("invalid {data_key}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if values.is_empty() || values.len() % 8 != 0 {
                return Err(format!("invalid {data_key} field count"));
            }
            self.buttons.insert(
                unit,
                values
                    .chunks_exact(8)
                    .map(|row| Button {
                        position: row[0],
                        enabled_string: row[6],
                        disabled_string: row[7],
                    })
                    .collect(),
            );
        }
        Ok(())
    }

    fn decode_tbl_overrides(
        &mut self,
        fields: &BTreeMap<String, E2RawValue>,
    ) -> Result<(), String> {
        let Some(count) = fields.get("stattextdicCount") else {
            return Ok(());
        };
        let count = raw_string(count)?
            .parse::<usize>()
            .map_err(|_| "invalid stattextdicCount".to_string())?;
        for index in 0..count {
            let key = raw_string(
                fields
                    .get(&format!("stattextdickey{index}"))
                    .ok_or_else(|| format!("missing stattextdickey{index}"))?,
            )?
            .parse::<u32>()
            .map_err(|_| format!("invalid stattextdickey{index}"))?;
            let value = raw_string(
                fields
                    .get(&format!("stattextdicvalue{index}"))
                    .ok_or_else(|| format!("missing stattextdicvalue{index}"))?,
            )?;
            if self._tbl_overrides.insert(key, value.to_string()).is_some() {
                return Err(format!("duplicate stat text {key}"));
            }
        }
        Ok(())
    }
}

fn raw_string(value: &E2RawValue) -> Result<&str, String> {
    match value {
        E2RawValue::String(value) => Ok(value),
        _ => Err("metadata value is not text".to_string()),
    }
}

fn raw_bool(value: &E2RawValue) -> Result<bool, String> {
    match value {
        E2RawValue::Bool(value) => Ok(*value),
        E2RawValue::String(value) if value.eq_ignore_ascii_case("true") => Ok(true),
        E2RawValue::String(value) if value.eq_ignore_ascii_case("false") => Ok(false),
        _ => Err("metadata value is not Boolean".to_string()),
    }
}

const DAT_BASES: [i64; 8] = [
    0x65FD00, 0x6564E0, 0x6C9858, 0x665AC0, 0x666778, 0x655700, 0x656198, 0x664A40,
];
const UNITS: &[u32] = &[
    0x000047F8, 0x40000AC0, 0x40000F38, 0x40004C80, 0x800013B0, 0x000008F0, 0x00004AB0, 0x40001100,
    0x80002650, 0x00003450, 0x000012C8, 0x000040D0, 0x000031A0, 0x00002568, 0x00004B98, 0x00003620,
    0x00003D50, 0x000039B8, 0x000048E0, 0x000019E0, 0x3FFFFF18, 0x00000478, 0x80004380, 0x000030B8,
    0x00003538, 0x000038D0, 0x00002480, 0x000001C8, 0x00002398, 0x400022C0, 0x400002B0, 0x40002EF0,
    0x40003E38, 0x400021E8, 0x40003F10, 0x40001740, 0x40002B60, 0x40002B62, 0x400029E0, 0x400029E2,
    0x40001AC8, 0x40001ACA, 0x40001ACC, 0x40001ACE, 0x40003288, 0x40003B88, 0x40000000, 0x40000728,
    0x40000D70, 0x00003AA0, 0x000049C8, 0x00003FE8, 0x00004710, 0x00000C88, 0x40003708, 0x400041B8,
    0x40000560, 0x000009D8, 0x40001818,
];
const WEAPONS: &[u32] = &[
    0x40000E00, 0x800007C8, 0x00000F08, 0x400014B8, 0x80000538, 0x80000F90, 0x00000CF0, 0x00000D78,
    0x00000190, 0x00000B60, 0x00000218, 0x400003A8, 0x40000BE8, 0x400012A0, 0x400009D0, 0x40001198,
    0x00000AD8, 0x00000000, 0x000004B0, 0x000013A8, 0x00001430, 0x00000740, 0x40000088, 0x400002A0,
];
const FLINGY: &[u32] = &[
    0x40000AC0, 0x800006A0, 0x40000420, 0x800000D8, 0x000005C8, 0x000009E8, 0x00000000,
];
const SPRITES: &[u32] = &[
    0x400006A0, 0x00000390, 0x00000AB0, 0x00000188, 0x00000000, 0x00000518,
];
const IMAGES: &[u32] = &[
    0x80002328, 0x000080E8, 0x000059D8, 0x00006D60, 0x00000FA0, 0x000036B0, 0x000032C8, 0x800084D0,
    0x80005DC0, 0x80004A38, 0x80003A98, 0x80001388, 0x80000000, 0x80007148,
];
const UPGRADES: &[u32] = &[
    0x40000040, 0x400002C0, 0x40000140, 0x400000C0, 0x40000480, 0x40000240, 0x400001C0, 0x400003C0,
    0x40000340, 0x000004FC, 0x00000000, 0x0000043C,
];
const TECHDATA: &[u32] = &[
    0x400000B0, 0x40000058, 0x40000240, 0x400001E8, 0x80000000, 0x40000298, 0x40000108, 0x000002F0,
    0x000001B8, 0x0000031C,
];
const ORDERS: &[u32] = &[
    0x40000840, 0x000000C0, 0x00000F00, 0x00000FC0, 0x00000000, 0x00000D80, 0x00000600, 0x000006C0,
    0x00000CC0, 0x00000780, 0x00000A80, 0x00000240, 0x00000180, 0x00000E40, 0x000003C0, 0x00000300,
    0x40000480, 0x40000B40, 0x000009C0,
];
const DAT_FIELDS: [&[u32]; 8] = [
    UNITS, WEAPONS, FLINGY, SPRITES, IMAGES, UPGRADES, TECHDATA, ORDERS,
];
const CUNIT_FIELDS: &[u32] = &[
    0x80000000, 0x80000004, 0x80000008, 0x8000000C, 0x40000010, 0x40000012, 0x80000014, 0x40000018,
    0x4000001A, 0x4000001C, 0x4000001E, 0x00000020, 0x00000021, 0x00000022, 0x00000023, 0x40000024,
    0x00000026, 0x00000027, 0x40000028, 0x4000002A, 0x8000002C, 0x80000030, 0x80000034, 0x80000038,
    0x8000003C, 0x80000040, 0x80000044, 0x40000048, 0x0000004A, 0x0000004B, 0x0000004C, 0x0000004D,
    0x0000004E, 0x0000004F, 0x40000050, 0x40000052, 0x00000054, 0x00000055, 0x00000056, 0x00000057,
    0x40000058, 0x4000005A, 0x8000005C, 0x80000060, 0x40000064, 0x40000066, 0x80000068, 0x8000006C,
    0x80000070, 0x80000074, 0x80000078, 0x8000007C, 0x80000080, 0x00000084, 0x00000085, 0x00000086,
    0x00000087, 0x40000088, 0x0000008A, 0x0000008B, 0x4000008C, 0x0000008E, 0x0000008F, 0x00000090,
    0x00000091, 0x00000092, 0x00000093, 0x40000094, 0x80000096, 0x80000097, 0x40000098, 0x4000009A,
    0x4000009C, 0x4000009E, 0x400000A0, 0x400000A2, 0x000000A4, 0x000000A5, 0x000000A6, 0x000000A7,
    0x400000A8, 0x400000AA, 0x400000AC, 0x400000AE, 0x400000B0, 0x400000B2, 0x400000B4, 0x400000B6,
    0x400000B8, 0x400000BA, 0x400000BC, 0x400000BE, 0x000000C0, 0x800000C0, 0x800000C4, 0x000000C8,
    0x000000C9, 0x800000C0, 0x800000C4, 0x800000C8, 0x800000CC, 0x800000C0, 0x800000C4, 0x800000C8,
    0x800000C0, 0x400000C4, 0x400000C6, 0x000000C8, 0x000000C9, 0x000000CA, 0x000000CB, 0x000000CC,
    0x000000CD, 0x400000CE, 0x800000C0, 0x400000C4, 0x400000C6, 0x800000C8, 0x400000CC, 0x800000CE,
    0x000000CF, 0x800000D0, 0x800000D4, 0x800000D8, 0x400000D0, 0x000000D2, 0x000000D3, 0x800000D4,
    0x000000D8, 0x000000D9, 0x800000D0, 0x800000D0, 0x800000D0, 0x800000D0, 0x800000D4, 0x400000D0,
    0x400000D2, 0x400000D4, 0x400000D6, 0x400000D0, 0x400000D2, 0x800000DC, 0x000000E0, 0x000000E1,
    0x000000E2, 0x000000E3, 0x800000E4, 0x400000E8, 0x400000EA, 0x800000EC, 0x800000F0, 0x800000F4,
    0x400000F8, 0x400000FA, 0x800000FC, 0x800000F8, 0x800000FC, 0x80000100, 0x00000104, 0x00000105,
    0x00000106, 0x80000107, 0x40000108, 0x4000010A, 0x4000010C, 0x4000010E, 0x40000110, 0x40000112,
    0x00000114, 0x00000115, 0x00000116, 0x00000117, 0x00000118, 0x00000119, 0x0000011A, 0x0000011B,
    0x8000011C, 0x00000120, 0x00000121, 0x00000122, 0x80000123, 0x00000124, 0x00000125, 0x00000126,
    0x00000127, 0x00000128, 0x00000129, 0x0000012A, 0x0000012B, 0x0000012C, 0x0000012D, 0x0000012E,
    0x0000012F, 0x40000130, 0x40000132, 0x80000134, 0x40000138, 0x4000013A, 0x0000013C, 0x00000140,
    0x00000144, 0x00000148, 0x0000014C, 0x0000014D, 0x0000014E, 0x0000014F,
];

fn unpack_offset(value: u32) -> i32 {
    ((value << 2) as i32) >> 2
}

fn unpack_width(value: u32) -> u8 {
    1 << (value >> 30)
}

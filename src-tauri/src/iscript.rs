//! Reader for StarCraft's `scripts\iscript.bin`: which animation slots a given
//! iscript actually defines.
//!
//! images.dat's `Iscript ID` selects an EXISTING script; the script's own header
//! says which animation slots it carries, and a slot whose offset is 0 has no
//! body at all. That is the fact a DAT edit needs before it repoints an image at
//! another script, and it exists nowhere but this file — the RAG corpus has the
//! slot NAME table and the ID→name list, not the per-script header.
//!
//! Layout (vendored MappingCore `Sc.h`, `IScriptDatFileHeader` /
//! `IScriptIdTableEntry` / `IScriptAnimationHeader`):
//!
//! ```text
//! offset 0 : u16 idTableOffset
//! idTable  : { u16 id; u16 headerOffset } ... terminated by id == 0xFFFF
//! header   : u32 "SCPE" | u16 lastAnimation | u16 unknown | u16 offsets[N]
//! ```
//!
//! `lastAnimation` is the HIGHEST animation index the script carries, not a
//! count, and the offset array is padded to an even length, so the array holds
//! `(lastAnimation + 2) & !1` entries.
//!
//! `Sc.h` names that field `animationCount` and documents the array as
//! `count & 0xFFFE` entries, which is short by two: under that reading 173 of
//! the 398 scripts in the retail file (every type 0 and type 1, including the
//! overlay scripts) come out with no animations at all, which no image could
//! run. The rule here is measured from the retail file instead — for types 1,
//! 12, 13, 14, 15, 20 and 21 the smallest distance between two consecutive
//! headers equals the padded length EXACTLY, and no type contradicts it.
//! `Slot::index` is therefore the real `AnimHeader` index, and a padded
//! trailing slot simply has no body.
//!
//! The parser is strict: a bad magic, a truncated header, an out-of-range slot
//! offset, or two headers whose slot arrays overlap is an error, never a quietly
//! shortened slot list. That overlap check is what keeps the array-length rule
//! honest against the real file.

use std::collections::BTreeMap;

/// Animation slot names, indexed by slot number (`Sc.h` `AnimHeader`).
pub const SLOT_NAMES: [&str; 28] = [
    "Init",
    "Death",
    "GroundAttackInit",
    "AirAttackInit",
    "Unused1",
    "GroundAttackRepeat",
    "AirAttackRepeat",
    "CastSpell",
    "GroundAttackToIdle",
    "AirAttackToIdle",
    "Unused2",
    "Walking",
    "WalkingToIdle",
    "SpecialState1",
    "SpecialState2",
    "AlmostBuilt",
    "Built",
    "Landing",
    "LiftOff",
    "IsWorking",
    "WorkingToIdle",
    "WarpIn",
    "Unused3",
    "StarEditInit",
    "Disable",
    "Burrow",
    "Unburrow",
    "Enable",
];

/// The archive-internal path of the script file inside the StarCraft data.
pub const ISCRIPT_ASSET: &str = r"scripts\iscript.bin";
/// The archive-internal path of the image GRP name table.
pub const IMAGES_TBL_ASSET: &str = r"arr\images.tbl";

const MAGIC: &[u8; 4] = b"SCPE";
/// magic(4) + type(2) + unknown(2), before the slot offset array.
const HEADER_FIXED: usize = 8;
const ID_TABLE_TERMINATOR: u16 = 0xFFFF;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub index: usize,
    pub name: String,
    pub present: bool,
    pub offset: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub id: u16,
    pub header_offset: u16,
    /// The header's raw type field — the highest animation index this script
    /// carries. Reported verbatim so a caller can see what the file declared
    /// rather than only this reader's interpretation of it.
    pub declared_type: u16,
    pub slots: Vec<Slot>,
}

impl Script {
    /// The slot names this script has no body for, in slot order.
    pub fn missing(&self) -> Vec<&str> {
        self.slots
            .iter()
            .filter(|slot| !slot.present)
            .map(|slot| slot.name.as_str())
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct Iscript {
    bytes: Vec<u8>,
    headers: BTreeMap<u16, u16>,
}

impl Iscript {
    /// Parse `iscript.bin`, validating every script header it declares.
    pub fn parse(bytes: Vec<u8>) -> Result<Self, String> {
        let table_offset = read_u16(&bytes, 0)
            .ok_or_else(|| "iscript.bin is too short to hold its ID table offset".to_string())?
            as usize;
        let mut headers = BTreeMap::new();
        let mut cursor = table_offset;
        let mut terminated = false;
        while let (Some(id), Some(offset)) =
            (read_u16(&bytes, cursor), read_u16(&bytes, cursor + 2))
        {
            if id == ID_TABLE_TERMINATOR {
                terminated = true;
                break;
            }
            // A duplicate id would make "which header is script N" ambiguous.
            if headers.insert(id, offset).is_some() {
                return Err(format!("iscript.bin declares script {id} twice"));
            }
            cursor += 4;
        }
        if !terminated {
            return Err("iscript.bin ID table is unterminated".to_string());
        }
        let parsed = Self { bytes, headers };
        parsed.validate()?;
        Ok(parsed)
    }

    /// Every script id the file declares, ascending.
    pub fn ids(&self) -> Vec<u16> {
        self.headers.keys().copied().collect()
    }

    pub fn script(&self, id: u16) -> Result<Script, String> {
        let header_offset = *self
            .headers
            .get(&id)
            .ok_or_else(|| format!("iscript.bin has no script {id}"))?;
        let (declared_type, slot_count) = self.header(header_offset)?;
        let base = header_offset as usize + HEADER_FIXED;
        let mut slots = Vec::with_capacity(slot_count);
        for index in 0..slot_count {
            let offset = read_u16(&self.bytes, base + index * 2)
                .ok_or_else(|| format!("iscript {id} header runs past the end of iscript.bin"))?;
            if offset != 0 && offset as usize >= self.bytes.len() {
                return Err(format!(
                    "iscript {id} slot {index} points past the end of iscript.bin"
                ));
            }
            slots.push(Slot {
                index,
                name: SLOT_NAMES
                    .get(index)
                    .map(|name| (*name).to_string())
                    .unwrap_or_else(|| format!("Unknown{index}")),
                present: offset != 0,
                offset,
            });
        }
        Ok(Script {
            id,
            header_offset,
            declared_type,
            slots,
        })
    }

    /// Read one header: returns its raw type field and the slot-array length.
    fn header(&self, header_offset: u16) -> Result<(u16, usize), String> {
        let start = header_offset as usize;
        let magic = self
            .bytes
            .get(start..start + 4)
            .ok_or_else(|| format!("iscript.bin header at {header_offset} is truncated"))?;
        if magic != MAGIC {
            return Err(format!(
                "iscript.bin header at {header_offset} is not an SCPE header"
            ));
        }
        let declared = read_u16(&self.bytes, start + 4)
            .ok_or_else(|| format!("iscript.bin header at {header_offset} is truncated"))?;
        // Slots 0..=declared, padded to an even array length (see the module
        // docs: measured from the retail file, not `Sc.h`'s `& 0xFFFE`). Kept in
        // usize so a corrupt 0xFFFF field overflows into the bounds check below
        // instead of wrapping to a small count.
        let slot_count = (declared as usize + 2) & !1_usize;
        let end = start + HEADER_FIXED + slot_count * 2;
        if end > self.bytes.len() {
            return Err(format!(
                "iscript.bin header at {header_offset} declares {slot_count} slots past the end of the file"
            ));
        }
        Ok((declared, slot_count))
    }

    /// Every header must be an SCPE header whose declared slot array fits in the
    /// file and stops before the next header begins. A slot count read too large
    /// would swallow the neighbouring header, so this is the check that proves
    /// the `count & 0xFFFE` rule against the real file instead of assuming it.
    fn validate(&self) -> Result<(), String> {
        let mut offsets: Vec<u16> = self.headers.values().copied().collect();
        offsets.sort_unstable();
        offsets.dedup();
        for (position, offset) in offsets.iter().enumerate() {
            let (_, slot_count) = self.header(*offset)?;
            let end = *offset as usize + HEADER_FIXED + slot_count * 2;
            if let Some(next) = offsets.get(position + 1) {
                if end > *next as usize {
                    return Err(format!(
                        "iscript.bin header at {offset} declares {slot_count} slots that overlap the header at {next}"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Byte length of each opcode's operands, indexed by opcode, as EUD Editor 3's
/// `AnimOpcodes.txt` documents them. `None` marks the two opcodes whose operand
/// list is a `u8` count followed by that many `u16` sound ids.
#[rustfmt::skip]
const OPERAND_BYTES: [Option<usize>; 0x45] = [
    Some(2), Some(2), Some(1), Some(1), Some(2), Some(1), Some(2), Some(2), // 00-07
    Some(4), Some(4), Some(2), Some(2), Some(0), Some(4), Some(4), Some(4), // 08-0f
    Some(4), Some(4), Some(2), Some(4), Some(4), Some(3), Some(0), Some(1), // 10-17
    Some(2), None,    Some(4), Some(0), None,    Some(0), Some(3), Some(1), // 18-1f
    Some(1), Some(0), Some(1), Some(1), Some(1), Some(1), Some(0), Some(0), // 20-27
    Some(1), Some(1), Some(0), Some(1), Some(1), Some(0), Some(0), Some(0), // 28-2f
    Some(0), Some(1), Some(0), Some(0), Some(1), Some(2), Some(0), Some(2), // 30-37
    Some(1), Some(2), Some(4), Some(6), Some(6), Some(2), Some(0), Some(2), // 38-3f
    Some(2), Some(1), Some(4), Some(0), Some(0),                            // 40-44
];

/// How many directions a turning GRP draws: frames `0..=16` of each frame set
/// face north through east to south, and the west half mirrors them.
const DIRECTIONS: u8 = 32;
/// Frames per frame set of a turning GRP (`engset` steps through whole sets).
const FRAMES_PER_SET: u16 = 17;
/// Instructions one animation may execute before it is cut off. Retail loops
/// close within a few dozen; the bound only stops a corrupt script.
const INSTRUCTION_LIMIT: usize = 20_000;
/// Timeline entries one animation may produce before it is cut off.
const STEP_LIMIT: usize = 1_024;

/// One frame held on screen for `ticks` game ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationStep {
    /// The GRP frame number, already adjusted for direction.
    pub frame: u16,
    /// Whether the frame is drawn mirrored (the west-facing half, or
    /// `setflipstate`).
    pub flip: bool,
    pub ticks: u16,
}

/// What one animation slot draws for one image, as a timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Animation {
    pub steps: Vec<AnimationStep>,
    /// The step the script loops back to. `None` means the script ends (an
    /// `end`, a `return` with nothing to return to) after the last step.
    pub loop_start: Option<usize>,
}

impl Iscript {
    /// Runs one animation slot of one script and records the frames it shows.
    ///
    /// This plays the image the way StarCraft plays it in isolation: `playfram`
    /// and its relatives choose the frame, `wait` holds it, `goto`/`call`/
    /// `return` steer, and the turn opcodes change the direction a turning GRP
    /// (`turns`) is drawn from. Everything that needs a live game — a target,
    /// an order, a random draw, another sprite or overlay, a sound — is not
    /// modelled: conditional jumps are never taken, `waitrand` waits its first
    /// value, and spawned overlays are not drawn. The first jump back into an
    /// instruction already run closes the loop.
    pub fn animate(
        &self,
        id: u16,
        slot: usize,
        turns: bool,
        direction: u8,
    ) -> Result<Animation, String> {
        let script = self.script(id)?;
        let entry = script
            .slots
            .get(slot)
            .filter(|slot| slot.present)
            .ok_or_else(|| format!("iscript {id} has no body for animation slot {slot}"))?;
        let mut pc = entry.offset as usize;
        let mut base: u16 = 0;
        let mut direction = direction % DIRECTIONS;
        let mut flipped = false;
        let mut stack: Vec<usize> = Vec::new();
        let mut steps: Vec<AnimationStep> = Vec::new();
        // The step count at the moment each instruction first ran.
        let mut seen: BTreeMap<usize, usize> = BTreeMap::new();

        let shown = |base: u16, direction: u8, flipped: bool| -> AnimationStep {
            if !turns {
                return AnimationStep {
                    frame: base,
                    flip: flipped,
                    ticks: 0,
                };
            }
            let (offset, mirrored) = if direction <= 16 {
                (u16::from(direction), false)
            } else {
                (u16::from(DIRECTIONS - direction), true)
            };
            AnimationStep {
                frame: base.saturating_add(offset),
                flip: mirrored != flipped,
                ticks: 0,
            }
        };

        for _ in 0..INSTRUCTION_LIMIT {
            if let Some(start) = seen.get(&pc) {
                // Back at an instruction already run: the animation repeats
                // from the step it had reached then. A cycle that never waited
                // simply holds the last frame.
                let loop_start = if *start < steps.len() {
                    *start
                } else {
                    steps.len().saturating_sub(1)
                };
                return Ok(finish(
                    steps,
                    Some(loop_start),
                    shown(base, direction, flipped),
                ));
            }
            seen.insert(pc, steps.len());

            let opcode = *self
                .bytes
                .get(pc)
                .ok_or_else(|| format!("iscript {id} runs past the end of iscript.bin at {pc}"))?;
            let width = match OPERAND_BYTES.get(opcode as usize) {
                Some(Some(width)) => *width,
                Some(None) => {
                    let count = *self
                        .bytes
                        .get(pc + 1)
                        .ok_or_else(|| format!("iscript {id} opcode at {pc} is truncated"))?;
                    1 + usize::from(count) * 2
                }
                None => {
                    return Err(format!(
                        "iscript {id} has an unknown opcode 0x{opcode:02x} at {pc}"
                    ))
                }
            };
            let operands = self
                .bytes
                .get(pc + 1..pc + 1 + width)
                .ok_or_else(|| format!("iscript {id} opcode at {pc} is truncated"))?;
            let byte = |index: usize| operands[index];
            let word = |index: usize| u16::from_le_bytes([operands[index], operands[index + 1]]);
            let next = pc + 1 + width;
            pc = next;

            match opcode {
                // playfram, playframtile, warpoverlay
                0x00 | 0x01 | 0x40 => base = word(0),
                // wait, waitrand (its first value: the wiki draws no random)
                0x05 | 0x06 => {
                    let ticks = u16::from(byte(0));
                    if ticks > 0 {
                        push(&mut steps, shown(base, direction, flipped), ticks);
                    }
                }
                0x07 => pc = usize::from(word(0)),
                0x16 => return Ok(finish(steps, None, shown(base, direction, flipped))),
                0x17 => flipped = byte(0) != 0,
                0x1f => direction = (direction + DIRECTIONS - byte(0) % DIRECTIONS) % DIRECTIONS,
                0x20 | 0x22 => direction = (direction + byte(0)) % DIRECTIONS,
                0x21 => direction = (direction + 1) % DIRECTIONS,
                0x2b => base = u16::from(byte(0)),
                0x2c => base = u16::from(byte(0)).saturating_mul(FRAMES_PER_SET),
                // ignorerest: nothing more happens until another animation.
                0x30 => {
                    let current = shown(base, direction, flipped);
                    if steps.last().map(|step| (step.frame, step.flip))
                        != Some((current.frame, current.flip))
                    {
                        push(&mut steps, current, 1);
                    }
                    let last = steps.len() - 1;
                    return Ok(finish(steps, Some(last), current));
                }
                0x34 => direction = byte(0) % DIRECTIONS,
                0x35 => {
                    stack.push(next);
                    pc = usize::from(word(0));
                }
                0x36 => match stack.pop() {
                    Some(back) => pc = back,
                    None => return Ok(finish(steps, None, shown(base, direction, flipped))),
                },
                _ => {}
            }
            if steps.len() >= STEP_LIMIT {
                return Ok(finish(steps, None, shown(base, direction, flipped)));
            }
        }
        Ok(finish(steps, None, shown(base, direction, flipped)))
    }
}

/// Appends a held frame. Steps are never merged: a loop may start at any of
/// them, and a merged step would move where the loop begins.
fn push(steps: &mut Vec<AnimationStep>, step: AnimationStep, ticks: u16) {
    steps.push(AnimationStep { ticks, ..step });
}

/// An animation that never waited still shows the frame it chose.
fn finish(
    mut steps: Vec<AnimationStep>,
    loop_start: Option<usize>,
    current: AnimationStep,
) -> Animation {
    if steps.is_empty() {
        steps.push(AnimationStep {
            ticks: 1,
            ..current
        });
        return Animation {
            steps,
            loop_start: Some(0),
        };
    }
    Animation { steps, loop_start }
}

/// Decode a StarCraft TBL string table (`u16 count`, `u16 offsets[count]`,
/// NUL-terminated strings). Used for `images.tbl`, whose entries are ASCII GRP
/// paths; bytes outside ASCII are kept lossily rather than guessed at.
pub fn tbl_strings(bytes: &[u8]) -> Result<Vec<String>, String> {
    let count = read_u16(bytes, 0).ok_or_else(|| "TBL is truncated".to_string())? as usize;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let start = read_u16(bytes, 2 + index * 2)
            .ok_or_else(|| "TBL offset table is truncated".to_string())?
            as usize;
        if start >= bytes.len() {
            return Err(format!("TBL offset {index} is invalid"));
        }
        let end = bytes[start..]
            .iter()
            .position(|value| *value == 0)
            .map(|relative| start + relative)
            .unwrap_or(bytes.len());
        values.push(String::from_utf8_lossy(&bytes[start..end]).into_owned());
    }
    Ok(values)
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an iscript.bin holding the given `(id, declared animationCount,
    /// slot offsets)` scripts, laid out header-after-header with the ID table
    /// last — the shape of the real file.
    fn synthetic(scripts: &[(u16, u16, Vec<u16>)]) -> Vec<u8> {
        let mut bytes = vec![0_u8; 2];
        // A body byte every nonzero slot offset can legitimately point at.
        bytes.push(0x05);
        let mut header_offsets = Vec::new();
        for (_, declared, offsets) in scripts {
            header_offsets.push(bytes.len() as u16);
            bytes.extend_from_slice(MAGIC);
            bytes.extend_from_slice(&declared.to_le_bytes());
            bytes.extend_from_slice(&0_u16.to_le_bytes());
            for offset in offsets {
                bytes.extend_from_slice(&offset.to_le_bytes());
            }
        }
        let table = bytes.len() as u16;
        for ((id, _, _), header) in scripts.iter().zip(&header_offsets) {
            bytes.extend_from_slice(&id.to_le_bytes());
            bytes.extend_from_slice(&header.to_le_bytes());
        }
        bytes.extend_from_slice(&ID_TABLE_TERMINATOR.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        let offset = table.to_le_bytes();
        bytes[0] = offset[0];
        bytes[1] = offset[1];
        bytes
    }

    #[test]
    fn reports_each_declared_slot_and_whether_it_has_a_body() {
        let iscript = Iscript::parse(synthetic(&[(225, 3, vec![2, 0, 2, 0])])).unwrap();
        let script = iscript.script(225).unwrap();

        assert_eq!(script.id, 225);
        assert_eq!(script.declared_type, 3);
        assert_eq!(script.slots.len(), 4);
        assert_eq!(script.slots[0].name, "Init");
        assert!(script.slots[0].present);
        assert_eq!(script.slots[1].name, "Death");
        assert!(!script.slots[1].present);
        assert_eq!(script.slots[2].name, "GroundAttackInit");
        assert!(script.slots[2].present);
        assert_eq!(script.missing(), vec!["Death", "AirAttackInit"]);
    }

    #[test]
    fn an_even_type_carries_one_padding_slot_and_an_odd_type_none() {
        // The type is the HIGHEST animation index and the array is padded to an
        // even length, so 12 and 13 both carry 14 entries: type 13 ends exactly
        // at SpecialState1, type 12 pads one slot past WalkingToIdle.
        let iscript =
            Iscript::parse(synthetic(&[(1, 12, vec![2; 14]), (2, 13, vec![2; 14])])).unwrap();

        let even = iscript.script(1).unwrap();
        assert_eq!(even.declared_type, 12);
        assert_eq!(even.slots.len(), 14);
        assert_eq!(even.slots[12].name, "WalkingToIdle");

        let odd = iscript.script(2).unwrap();
        assert_eq!(odd.declared_type, 13);
        assert_eq!(odd.slots.len(), 14);
        assert_eq!(odd.slots.last().unwrap().name, "SpecialState1");
    }

    #[test]
    fn the_full_28_slot_script_names_every_animation() {
        let iscript = Iscript::parse(synthetic(&[(0, 27, vec![2; 28])])).unwrap();
        let script = iscript.script(0).unwrap();

        assert_eq!(script.slots.len(), 28);
        assert_eq!(script.slots[7].name, "CastSpell");
        assert_eq!(script.slots[24].name, "Disable");
        assert_eq!(script.slots[27].name, "Enable");
        assert!(script.missing().is_empty());
    }

    #[test]
    fn every_declared_script_is_listed() {
        let iscript = Iscript::parse(synthetic(&[
            (0, 0, vec![2, 2]),
            (225, 0, vec![2, 2]),
            (12, 0, vec![2, 2]),
        ]))
        .unwrap();

        assert_eq!(iscript.ids(), vec![0, 12, 225]);
        assert_eq!(
            iscript.script(400).unwrap_err(),
            "iscript.bin has no script 400"
        );
    }

    #[test]
    fn a_header_without_the_scpe_magic_is_refused() {
        let mut bytes = synthetic(&[(225, 0, vec![2, 2])]);
        bytes[3] = b'X';

        assert!(Iscript::parse(bytes)
            .unwrap_err()
            .contains("is not an SCPE header"));
    }

    #[test]
    fn a_slot_array_that_overlaps_the_next_header_is_refused() {
        // Two adjacent headers where the first claims more slots than it owns:
        // reading it as declared would return the next header's bytes as slots.
        let mut bytes = synthetic(&[(1, 0, vec![2, 2]), (2, 0, vec![2, 2])]);
        let first_type = 3 + 4; // body byte + first header magic
        bytes[first_type] = 8; // type 8 -> 10 slots, past the next header
        bytes[first_type + 1] = 0;

        assert!(Iscript::parse(bytes).unwrap_err().contains("overlap"));
    }

    #[test]
    fn an_unterminated_id_table_is_refused() {
        let mut bytes = synthetic(&[(225, 0, vec![2, 2])]);
        let length = bytes.len();
        bytes.truncate(length - 4);

        assert_eq!(
            Iscript::parse(bytes).unwrap_err(),
            "iscript.bin ID table is unterminated"
        );
    }

    #[test]
    fn a_slot_offset_past_the_end_of_the_file_is_refused() {
        let iscript = Iscript::parse(synthetic(&[(225, 0, vec![2, 0xFFF0])])).unwrap();

        assert!(iscript
            .script(225)
            .unwrap_err()
            .contains("points past the end"));
    }

    /// The synthetic fixtures above prove the reader against a file this module
    /// wrote itself. This one proves the FORMAT against Blizzard's real
    /// `iscript.bin`, which is the only thing that can: `parse` rejects any
    /// header whose slot array runs into the next header, and every script in
    /// the retail file must have an Init body to run at all. `Sc.h`'s documented
    /// `count & 0xFFFE` array length passes the first check and fails the second
    /// on 173 of the 398 scripts, which is how the off-by-two was found.
    #[test]
    #[ignore = "requires installed StarCraft data"]
    fn every_retail_script_parses_with_an_init_body_inside_the_named_slots() {
        let starcraft = std::path::Path::new(r"C:\Program Files (x86)\StarCraft");
        let bytes = isom::game_asset(starcraft, ISCRIPT_ASSET).unwrap();
        let iscript = Iscript::parse(bytes).unwrap();

        let ids = iscript.ids();
        assert!(
            ids.len() > 100,
            "retail iscript.bin declares {} scripts",
            ids.len()
        );
        for id in ids {
            let script = iscript.script(id).unwrap();
            assert_eq!(
                script.slots.len(),
                (script.declared_type as usize + 2) & !1_usize,
                "script {id} slot count"
            );
            assert!(
                script.slots.len() <= SLOT_NAMES.len(),
                "script {id} declares {} slots, past the {} named animations",
                script.slots.len(),
                SLOT_NAMES.len()
            );
            assert!(
                script.slots[0].present,
                "script {id} (type {}) has no Init body",
                script.declared_type
            );
        }
    }

    /// One script (id 7) whose Init body is `body`, placed at offset 2 so a
    /// test can write absolute jump targets as `2 + index`.
    fn with_body(body: &[u8]) -> Iscript {
        let mut bytes = vec![0_u8; 2];
        bytes.extend_from_slice(body);
        let header = bytes.len() as u16;
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        let table = bytes.len() as u16;
        bytes.extend_from_slice(&7_u16.to_le_bytes());
        bytes.extend_from_slice(&header.to_le_bytes());
        bytes.extend_from_slice(&ID_TABLE_TERMINATOR.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes[..2].copy_from_slice(&table.to_le_bytes());
        Iscript::parse(bytes).unwrap()
    }

    fn step(frame: u16, flip: bool, ticks: u16) -> AnimationStep {
        AnimationStep { frame, flip, ticks }
    }

    #[test]
    fn a_goto_back_into_the_body_loops_from_the_step_it_had_reached() {
        let iscript = with_body(&[
            0x00, 5, 0, // 2: playfram 5
            0x05, 3, // 5: wait 3
            0x00, 6, 0, // 7: playfram 6
            0x05, 2, // 10: wait 2
            0x00, 7, 0, // 12: playfram 7
            0x05, 1, // 15: wait 1
            0x07, 7, 0, // 17: goto 7
        ]);
        let animation = iscript.animate(7, 0, false, 0).unwrap();
        assert_eq!(
            animation.steps,
            vec![step(5, false, 3), step(6, false, 2), step(7, false, 1)]
        );
        assert_eq!(animation.loop_start, Some(1), "the intro frame plays once");
    }

    #[test]
    fn a_turning_graphic_draws_its_direction_and_mirrors_the_west_half() {
        let iscript = with_body(&[
            0x00, 34, 0, // 2: playfram 34 (frame set 2)
            0x05, 1,    // 5: wait 1
            0x21, // 7: turn1cwise
            0x05, 1,    // 8: wait 1
            0x16, // 10: end
        ]);
        let east = iscript.animate(7, 0, true, 8).unwrap();
        assert_eq!(east.steps, vec![step(42, false, 1), step(43, false, 1)]);
        assert_eq!(east.loop_start, None, "`end` finishes the animation");

        // Direction 20 is the mirror of direction 12.
        let west = iscript.animate(7, 0, true, 20).unwrap();
        assert_eq!(west.steps, vec![step(46, true, 1), step(45, true, 1)]);

        let fixed = iscript.animate(7, 0, false, 20).unwrap();
        assert_eq!(fixed.steps, vec![step(34, false, 1), step(34, false, 1)]);
    }

    #[test]
    fn a_call_returns_and_a_conditional_jump_is_never_taken() {
        let iscript = with_body(&[
            0x35, 12, 0, // 2: call 12
            0x1e, 255, 2, 0, // 5: randcondjmp 255 2
            0x05, 4,    // 9: wait 4
            0x30, // 11: ignorerest
            0x00, 9, 0,    // 12: playfram 9
            0x36, // 15: return
        ]);
        let animation = iscript.animate(7, 0, false, 0).unwrap();
        assert_eq!(animation.steps, vec![step(9, false, 4)]);
        assert_eq!(animation.loop_start, Some(0), "ignorerest holds the frame");
    }

    #[test]
    fn a_body_that_never_waits_still_shows_its_frame() {
        let iscript = with_body(&[0x00, 3, 0, 0x07, 2, 0]);
        let animation = iscript.animate(7, 0, false, 0).unwrap();
        assert_eq!(animation.steps, vec![step(3, false, 1)]);
        assert_eq!(animation.loop_start, Some(0));
    }

    #[test]
    fn an_unknown_opcode_or_missing_slot_is_refused() {
        let iscript = with_body(&[0x00, 3, 0, 0x7f]);
        let error = iscript.animate(7, 0, false, 0).unwrap_err();
        assert!(error.contains("0x7f"), "{error}");
        let error = iscript.animate(7, 1, false, 0).unwrap_err();
        assert!(error.contains("slot 1"), "{error}");
    }

    /// Every retail animation must play to a loop or an end: an operand table
    /// off by one byte desynchronises the stream and lands on an opcode past
    /// 0x44 within a few instructions.
    #[test]
    #[ignore = "requires installed StarCraft data"]
    fn every_retail_animation_plays() {
        let starcraft = std::path::Path::new(r"C:\Program Files (x86)\StarCraft");
        let iscript = Iscript::parse(isom::game_asset(starcraft, ISCRIPT_ASSET).unwrap()).unwrap();
        let mut played = 0;
        for id in iscript.ids() {
            for slot in iscript.script(id).unwrap().slots {
                if !slot.present {
                    continue;
                }
                let animation = iscript
                    .animate(id, slot.index, true, 12)
                    .unwrap_or_else(|error| panic!("script {id} {}: {error}", slot.name));
                assert!(!animation.steps.is_empty());
                played += 1;
            }
        }
        assert!(played > 1000, "{played} animations");
    }

    #[test]
    fn tbl_strings_reads_each_nul_terminated_entry() {
        let mut bytes = 2_u16.to_le_bytes().to_vec();
        let first = 6_u16;
        let second = first + b"unit\\terran\\marine.grp\0".len() as u16;
        bytes.extend_from_slice(&first.to_le_bytes());
        bytes.extend_from_slice(&second.to_le_bytes());
        bytes.extend_from_slice(b"unit\\terran\\marine.grp\0");
        bytes.extend_from_slice(b"neutral\\geyser.grp\0");

        assert_eq!(
            tbl_strings(&bytes).unwrap(),
            vec![
                "unit\\terran\\marine.grp".to_string(),
                "neutral\\geyser.grp".to_string()
            ]
        );
    }
}

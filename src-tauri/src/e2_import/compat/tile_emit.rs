pub(super) fn render_halfword(base: &str, position: u32, value: u32, output: &mut String) {
    let word = position / 4;
    let tag = format!("{}_{}", base.trim_end_matches("offset"), position);
    break_word(&tag, &format!("f_dwread_epd({base} + {word})"), output);
    let expression = if position % 4 == 0 {
        format!("{tag}b2 * 65536 + {tag}b3 * 16777216 + {value}")
    } else {
        format!("{tag}b0 + {tag}b1 * 256 + {}", value * 65536)
    };
    output.push_str(&format!(
        "SetDeaths({base} + {word}, SetTo, {expression}, 0);\n"
    ));
}

pub(super) fn render_existing_tile(tile: u16, bytes: &[u8], serial: usize, output: &mut String) {
    let position = 0x14 + 52 * u32::from(tile / 16) + 2 * u32::from(tile % 16);
    let cv5tag = format!("cv5_{serial}");
    break_word(
        &cv5tag,
        &format!("f_dwread_epd(cv5offset + {})", position / 4),
        output,
    );
    let cv5value = halfword(&cv5tag, position);
    output.push_str(&format!("var cv5tile{serial} = {cv5value};\n"));
    for mini in 0..16usize {
        let byte = mini * 2;
        let tag = format!("vx4_{serial}_{mini}");
        break_word(
            &tag,
            &format!(
                "f_dwread_epd(vx4offset + cv5tile{serial} * 8 + {})",
                byte / 4
            ),
            output,
        );
        let value = halfword(&tag, u32::try_from(byte).expect("mini offset fits u32"));
        output.push_str(&format!("var vx4value{serial}_{mini} = {value};\n"));
        output.push_str(&format!(
            "var vr4index{serial}_{mini} = (vx4value{serial}_{mini} & 65534) / 2;\n"
        ));
        output.push_str(&format!("if (vx4value{serial}_{mini} & 1) {{\n"));
        super::tile::render_pixels(
            &format!("vr4offset + vr4index{serial}_{mini} * 16"),
            bytes,
            mini * 64,
            true,
            output,
        );
        output.push_str("} else {\n");
        super::tile::render_pixels(
            &format!("vr4offset + vr4index{serial}_{mini} * 16"),
            bytes,
            mini * 64,
            false,
            output,
        );
        output.push_str("}\n");
    }
}

fn break_word(tag: &str, value: &str, output: &mut String) {
    output.push_str(&format!(
        "var {tag}b0, {tag}b1, {tag}b2, {tag}b3 = f_dwbreak({value});\n"
    ));
}

fn halfword(tag: &str, position: u32) -> String {
    if position % 4 == 0 {
        format!("{tag}b0 + {tag}b1 * 256")
    } else {
        format!("{tag}b2 + {tag}b3 * 256")
    }
}

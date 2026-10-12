use std::collections::BTreeMap;

use super::super::{E2CompiledSources, E2ImportIr, E2ImportIssue};
use super::tree::Renderer;
use super::CompileContext;

const BGM_LIFECYCLE_MARKER: &str = "// E2-BGM-LIFECYCLE";

pub(super) fn compile(
    document: &E2ImportIr,
    context: &CompileContext,
) -> Result<E2CompiledSources, E2ImportIssue> {
    let mut renderer = Renderer::new(document, context)?;
    for node in &document.triggers {
        renderer.top(node)?;
    }
    let wait_enabled = renderer.wait.has_enabled();
    let raw_enabled = renderer.raw.has_triggers();
    let wait_module = renderer.wait.module();
    let raw_module = renderer.raw.module();
    let mut before = String::new();
    if wait_enabled || raw_enabled {
        line(&mut before, 1, "EUDPlayerLoop()();");
        if wait_enabled {
            line(&mut before, 1, "WaitableTriggerExec();");
        }
        if raw_enabled {
            line(&mut before, 1, "ClassicTriggerExec();");
        }
        line(&mut before, 1, "EUDEndPlayerLoop();");
        if raw_enabled {
            line(&mut before, 1, "ClassicTriggerStarter();");
        }
    }
    line(&mut before, 1, BGM_LIFECYCLE_MARKER);
    before.push_str(&renderer.hooks[1]);
    let bgm_import = if renderer.needs_bgm {
        "import e2.bgm as BGM;\n"
    } else {
        ""
    };
    let main = format!(
        "import e2.tct as tct;\n{bgm_import}\n{}\n{wait_module}{raw_module}\nfunction onPluginStart() {{\n{}}}\n\nfunction beforeTriggerExec() {{\n{before}}}\n\nfunction afterTriggerExec() {{\n{}}}\n",
        renderer.provenance, renderer.hooks[0], renderer.hooks[2]
    );
    let mut files = BTreeMap::new();
    files.insert("src/e2/tct.py".to_string(), helpers());
    files.insert("src/main.eps".to_string(), main);
    Ok(E2CompiledSources { files })
}

pub(super) fn helpers() -> String {
    r#"# Narrow E2 custom-text compatibility surface.
# Adapted from customText 0.5.2 by Artanis in armoha/EUDEditor v0.17.9.7.
# MIT License, Copyright (c) 2018 LeeJungHun. See LICENSE.EUD-Editor-2-customText.
from eudplib import *

_sb = StringBuffer(1023)

def f_makeText(*args):
    _sb.insert(0)
    _sb.append(*args)

def f_chatAnnouncement(*args):
    f_eprintln(*args)

@EUDFunc
def _findTblSegment(tblID, segment):
    ptr = GetTBLAddr(tblID)
    offset, seen = EUDCreateVariables(2)
    if EUDWhile()(f_bread(ptr + offset) != 0):
        if EUDIf()(f_bread(ptr + offset) == 35):
            seen += 1
            if EUDIf()(seen == segment):
                EUDReturn(offset)
            EUDEndIf()
            if EUDWhile()(f_bread(ptr + offset) == 35):
                offset += 1
            EUDEndWhile()
        if EUDElse()():
            offset += 1
        EUDEndIf()
    EUDEndWhile()
    EUDReturn(0)

def f_setTbl(tblID, segment, *args):
    ptr = GetTBLAddr(tblID)
    offset = _findTblSegment(tblID, segment)
    start = offset
    if EUDWhile()(f_bread(ptr + offset) == 35):
        f_bwrite(ptr + offset, 13)
        offset += 1
    EUDEndWhile()
    f_settbl2(tblID, start, *args)

f_addText = _sb.append
f_displayText = _sb.Display
f_print = _sb.print
f_printError = f_raise_CCMU

addText = f_addText
chatAnnouncement = f_chatAnnouncement
displayText = f_displayText
makeText = f_makeText
print = f_print
printError = f_printError
setTbl = f_setTbl
"#
    .to_string()
}

pub(super) fn line(out: &mut String, depth: usize, text: &str) {
    out.push_str(&"    ".repeat(depth));
    out.push_str(text);
    out.push('\n');
}

#!/usr/bin/env python3
"""Split one Rust module file into a directory: one top-level named item per file.

Rule (the agreed strict split):
  * Every named top-level item -> its own file, named for the item.
  * A type's own inherent `impl`, trait impls (`impl Trait for T`), type aliases
    naming it, and serde `default`/helper fns used only by it -> travel into the
    type's file.
  * Private free fns that are tiny single-caller helpers may travel (caller lists
    them in TRAVEL); everything else gets a file.
  * `mod tests` is flattened into tests.rs (raw strings kept verbatim).
  * The crate/module doc comment goes to mod.rs.

Usage: splitmod.py <file.rs> [--ns FILE]...
  --ns FILE  treat these whole items as a cohesive namespace kept in their own
             single file (e.g. a flat const-name namespace like kind.rs).
"""
import re, sys, os, json

KEYWORDS = ('fn', 'struct', 'enum', 'trait', 'type', 'const', 'static', 'impl',
            'union', 'macro')

def parse_items(src):
    lines = src.split('\n')
    N = len(lines)
    def doc_back(i):
        j = i
        while j - 1 >= 0:
            s = lines[j-1].strip()
            if s.startswith('///') or s.startswith('#![') or \
               (s.startswith('#[') and s != '#[cfg(test)]'):
                j -= 1
            else:
                break
        return j
    def find_end(i):
        k, depth, started = i, 0, False
        while k < N:
            for ch in lines[k]:
                if ch == '{':
                    depth += 1; started = True
                elif ch == '}':
                    depth -= 1
            # enum/struct/union with `;` and no braces, or type/const ending `;`
            if not started and re.search(r'[;]$', lines[k].strip()):
                return k
            if started and depth == 0:
                return k
            k += 1
        return N - 1
    items = []
    macro_rules_defs = []
    i = 0
    while i < N:
        # macro_rules! definition: keep in mod.rs (module-scoped, textual order)
        if re.match(r'^macro_rules!\s+\w+', lines[i]):
            k = i; depth = 0; started = False
            while k < N:
                for ch in lines[k]:
                    if ch in '({[': depth += 1; started = True
                    elif ch in ')}]': depth -= 1
                if started and depth <= 0: break
                k += 1
            macro_rules_defs.append('\n'.join(lines[i:k+1]))
            i = k + 1
            continue
        # type-defining macro calls: `resource!(...)`, `resource_kind!(...)`, etc.
        mm = re.match(r'^(?:pub )?(\w+)\s*!\s*[\(\{\[]', lines[i])
        if mm and mm.group(1) != 'macro_rules':
            block = []
            k = i; depth = 0; started = False
            while k < N:
                block.append(lines[k])
                for ch in lines[k]:
                    if ch in '({[':
                        depth += 1; started = True
                    elif ch in ')}]':
                        depth -= 1
                if started and depth <= 0:
                    break
                k += 1
            body = '\n'.join(block)
            nm = re.search(r'!\s*\(?\s*((?://[/!][^\n]*\n\s*)*)\b([A-Z]\w*)', body)
            typename = nm.group(2) if nm else None
            if typename:
                items.append({'kind': 'macro', 'name': typename, 'target': typename,
                              'start': doc_back(i), 'end': k})
                i = k + 1
                continue
        m = re.match(r'^((?:pub(?:\([a-z]+\))? )?(?:pub )?(?:const |static )?(?:async )?)'
                     r'(fn|struct|enum|impl|trait|type|const|static|union|macro)\b(.*)',
                     lines[i])
        if m:
            kind = m.group(2); rest = m.group(3)
            if kind == 'impl':
                if ' for ' in rest:
                    name = 'impl:' + rest.split(' for ')[-1].split('{')[0].strip()
                    target = re.search(r'for\s+(\w+)', rest).group(1)
                else:
                    mm = re.match(r'\s*(?:<[^>]*>\s*)?(\w+)', rest)
                    name = 'impl:' + (mm.group(1) if mm else 'anon')
                    target = mm.group(1) if mm else 'anon'
            else:
                nm = re.match(r'\s*(\w+)', rest)
                name = nm.group(1) if nm else ''
                target = name
            s = doc_back(i); e = find_end(i)
            items.append({'kind': kind, 'name': name, 'target': target,
                          'start': s, 'end': e})
            i = e + 1
            continue
        if lines[i].strip() == '#[cfg(test)]' and i + 1 < N and 'mod tests' in lines[i+1]:
            break
        i += 1
    return items, lines, macro_rules_defs

def snake(name):
    if name.isupper() or re.match(r'^[A-Z0-9_]+$', name):
        return name.lower()
    # split camel/PascalCase into snake; keep existing underscores
    s = re.sub(r'(?<!^)(?=[A-Z])', '_', name).lower()
    return s


def prune_unused_uses(src):
    """Drop import tokens not referenced anywhere in the file body.
    Keeps `use super::*;` and trait-only imports we cannot see used by name
    (best effort: only removes leaf names absent from the whole file text)."""
    lines = src.split('\n')
    # body text = everything that is not a top-level use statement
    body_parts = []
    i = 0
    n = len(lines)
    use_idx = set()
    while i < n:
        if re.match(r'^use\b', lines[i]):
            depth = 0
            j = i
            while j < n:
                use_idx.add(j)
                depth += lines[j].count('{') - lines[j].count('}')
                if depth <= 0 and lines[j].rstrip().endswith(';'):
                    break
                j += 1
            i = j + 1
            continue
        body_parts.append(lines[i])
        i += 1
    body_text = '\n'.join(body_parts)
    # also count a leaf as "used" if it appears in body OR in any attribute line
    out = []
    for k, line in enumerate(lines):
        if k not in use_idx:
            out.append(line)
            continue
        stripped = line.strip()
        if stripped in ('use super::*;',) or '::\\*' in stripped or 'use super::*' in stripped:
            out.append(line); continue
        # single-line `use a::b;`
        m1 = re.match(r'^use ([\w:]+::)(\w+)( as _)?;$', stripped)
        if m1:
            if re.search(r'(?<!\w)'+re.escape(m1.group(2))+r'\b', body_text):
                out.append(line)
            continue
        # brace single-line `use a::{b, c};`
        m2 = re.match(r'^use ([\w:]+::)\{(.*)\};$', stripped)
        if m2 and '{' in stripped and stripped.endswith('};') and stripped.count('{') == 1:
            items = [x.strip() for x in m2.group(2).split(',') if x.strip()]
            kept = [x for x in items
                    if re.search(r'(?<!\w)'+re.escape(x.split(' as ')[0].strip())+r'\b', body_text)]
            if not kept:
                continue
            out.append('use %s{%s};' % (m2.group(1), ', '.join(kept)))
            continue
        # multi-line brace use: pass through (conservative; cargo/clippy handle these)
        out.append(line)
    return '\n'.join(out)

def main():
    path = sys.argv[1]
    ns_files = set()
    for a in sys.argv[2:]:
        if a == '--ns':
            continue
    # collect --ns args
    args = sys.argv[2:]
    ns_items = set()
    it = iter(range(len(args)))
    j = 0
    while j < len(args):
        if args[j] == '--ns':
            ns_items.add(args[j+1]); j += 2
        else:
            j += 1
    src = open(path).read()
    items, lines, macro_rules_defs = parse_items(src)
    # locate test module up front
    test_marker = None
    for idx, l in enumerate(lines):
        if l.strip() == '#[cfg(test)]' and idx+1 < len(lines) and 'mod tests' in lines[idx+1]:
            test_marker = idx
            break
    has_tests = test_marker is not None
    moddir = path[:-3]
    os.makedirs(moddir, exist_ok=True)
    HDR = ('// SPDX-License-Identifier: Apache-2.0\n'
           '// Copyright (c) 2026 P. Industries LLC and PiStack contributors\n\n')

    # use block: gather top-level `use` statements (with continuation lines).
    ub = None
    for idx, l in enumerate(lines):
        if l.startswith('use '):
            ub = idx
            break
    use_block = ''
    if ub is not None:
        k = ub
        depth = 0
        block = []
        # Collect every top-level `use`/`pub use` statement from the import
        # header, allowing blank/comment lines to separate groups, until the
        # first line that is not part of a use statement.
        while k < len(lines):
            l = lines[k]
            s = l.strip()
            if depth == 0:
                # only start/continue the block on use lines (plus blanks/plain
                # comments between groups), until the first item doc or decl.
                if s == '' or s.startswith('//'):
                    # an item doc comment (///) ends the import header
                    if s.startswith('///'):
                        break
                    if block:
                        block.append(l)
                    k += 1
                    continue
                if not (s.startswith('use ') or s.startswith('pub use ')):
                    break
            block.append(l)
            depth += l.count('{') - l.count('}') + l.count('(') - l.count(')')
            if depth <= 0 and l.rstrip().endswith(';'):
                depth = 0
            k += 1
        # drop trailing blanks
        while block and block[-1].strip() == '':
            block.pop()
        use_block = '\n'.join(block).strip('\n')

    # module doc comment: //! lines at top
    doc_lines = []
    for l in lines[:ub if ub is not None else 10]:
        if l.startswith('//!'):
            doc_lines.append(l)
    mod_doc = '\n'.join(doc_lines)

    # group items: type file owns its impls/aliases named after it
    # base name for files
    def file_for(item):
        if item['kind'] == 'impl':
            return snake(item['target'])
        return snake(item['name'])

    # first pass: which type-names exist as structs/enums/unions/macros?
    type_names = {i['name'] for i in items if i['kind'] in ('struct','enum','union','macro')}

    groups = {}   # filename -> list of items (in original order)
    order = sorted(items, key=lambda x: x['start'])
    for item in order:
        if item['kind'] == 'impl':
            t = item['target']
            fn = snake(t)
        elif item['kind'] == 'macro':
            fn = snake(item['target'])
        elif item['kind'] == 'type':
            # alias travels to the type it names if that type is a real item; else own file
            target_alias = re.search(r'=\s*(\w+)', lines[item['start']])
            base = target_alias.group(1) if target_alias else item['name']
            fn = snake(base) if base in type_names else snake(item['name'])
        else:
            fn = snake(item['name'])
        groups.setdefault(fn, []).append(item)

    # namespaces: if --ns ITEM given, that item's file is kept whole (the item + any
    # const in the same run). Handled by caller listing the anchor; here just ensure
    # each ns item is its own file (default behavior already).
    allnames = set()
    for grp in groups.values():
        for it in grp:
            if it['kind'] != 'impl':
                allnames.add(it['name'])

    # write files
    def body_of(item):
        return '\n'.join(lines[item['start']:item['end']+1])

    written = {}
    for fn, grp in groups.items():
        grp_sorted = sorted(grp, key=lambda x: x['start'])
        body = '\n\n'.join(body_of(i) for i in grp_sorted)
        written[fn] = body

    # A child file must not share its directory's name (`client/client.rs`):
    # rustc/clippy reject "module has the same name as its containing module".
    dirname = os.path.basename(moddir.rstrip('/'))
    if dirname in written and dirname != 'mod':
        renamed = written.pop(dirname)
        written[dirname + '_type'] = renamed
        # remap the owning group so imports/re-exports follow.
        groups[dirname + '_type'] = groups.pop(dirname)

    # imports for a file: references to other files' owned names
    file_names = {}  # filename -> set of public names it defines
    for fn, grp in groups.items():
        names = set()
        for i in grp:
            if i['kind'] != 'impl':
                names.add(i['name'])
            else:
                names.add(i['target'])
        file_names[fn] = names

    # Which (file, item-name) are referenced from a *different* file?
    crossref = {}
    for fn, body in written.items():
        for ofn, onames in file_names.items():
            if ofn == fn:
                continue
            for nm in onames:
                if re.search(r'(?<!\w)'+re.escape(nm)+r'\b', body):
                    crossref.setdefault(ofn, set()).add(nm)

    def promote_in(fn, body):
        """Module-internal file: promote private fields + inherent methods to
        pub(crate). Safe because these files are already pub(crate)-scoped."""
        out = []
        cur_struct = None
        cur_impl = None
        depth = 0          # running brace depth
        body_entry = None  # depth value at entry into current struct/impl (0 for top-level)
        need = crossref.get(fn, set())
        for ln in body.split('\n'):
            s = ln.strip()
            opens = ln.count('{')
            closes = ln.count('}')
            entering = ln.rstrip().endswith('{')
            ms = re.match(r'^(?:pub(?:\(\w+\))? )?(?:struct|enum|union)\s+(\w+)', ln)
            mi = re.match(r'^impl\s+(?:<[^>]*>\s*)?(\w+)', ln) if ' for ' not in ln else None
            is_top = (depth == 0)
            if is_top and ms and entering:
                cur_struct = ms.group(1)
                if cur_struct in need and re.match(r'^(struct|enum|union)\b', ln):
                    ln = 'pub(crate) ' + ln
            elif is_top and mi and entering:
                cur_impl = mi.group(1)
            # promotions inside current body
            if cur_struct and depth >= 1 and re.match(r'^    [a-z_][a-z0-9_]*\s*:', ln) and not s.startswith('#['):
                ln = re.sub(r'^    ', '    pub(crate) ', ln)
            if cur_impl and depth >= 1 and re.match(r'^    (async )?fn ', ln):
                ln = re.sub(r'^    ', '    pub(crate) ', ln)
            # top-level private decl referenced elsewhere
            if is_top and not cur_struct and not cur_impl:
                md = re.match(r'^(?:async )?(fn|struct|enum|trait|type|const|static|union)\s+(\w+)', ln)
                if md and md.group(2) in need:
                    ln = 'pub(crate) ' + ln
            depth += opens - closes
            if depth <= 0:
                cur_struct = None; cur_impl = None
            out.append(ln)
        return '\n'.join(out)


    for fn, body in written.items():
        refs = {}
        for ofn, onames in file_names.items():
            if ofn == fn:
                continue
            for nm in onames:
                if re.search(r'(?<!\w)'+re.escape(nm)+r'\b', body):
                    refs.setdefault(ofn, set()).add(nm)
        body = promote_in(fn, body)
        out = HDR
        if use_block:
            out += use_block + '\n\n'
        if refs:
            imp = []
            for ofn in sorted(refs):
                imp.append('use super::%s::{%s};' % (ofn, ', '.join(sorted(refs[ofn]))))
            out += '\n'.join(imp) + '\n\n'
        out += body.rstrip() + '\n'
        open(os.path.join(moddir, fn + '.rs'), 'w').write(out)

    # mod.rs
    mods = sorted(groups.keys())
    modrs = HDR
    if mod_doc:
        modrs += mod_doc + '\n\n'
    if macro_rules_defs:
        modrs += '\n\n'.join(macro_rules_defs) + '\n\n'
    modrs += '\n'.join('mod %s;' % m for m in mods) + '\n'
    if has_tests:
        modrs += '#[cfg(test)]\nmod tests;\n'
    # re-export public + pub(crate) items so callers keep `module::item` paths.
    reexport = []
    for fn, grp in groups.items():
        for i in sorted(grp, key=lambda x: x['start']):
            if i['kind'] == 'impl':
                continue
            if i['kind'] == 'macro':
                # a resource!-style call defines a public type; re-export it.
                reexport.append(('pub', fn, i['name']))
                continue
            first = lines[i['start']:i['end']+1]
            declline = next((ln for ln in first
                             if re.match(r'^(pub(\(\w+\))? )?(pub )?(async )?(fn|struct|enum|trait|type|const|static|union)\b', ln)), '')
            mvis = re.match(r'^pub\((\w+)\)', declline)
            if declline.startswith('pub ') or re.match(r'^pub (async )?(fn|struct|enum)', declline):
                reexport.append(('pub', fn, i['name']))
            elif mvis:
                reexport.append(('pub(%s)' % mvis.group(1), fn, i['name']))
    seen = set()
    for vis, fn, nm in reexport:
        key = (fn, nm)
        if key in seen:
            continue
        seen.add(key)
        modrs += '%s use %s::%s;\n' % (vis, fn, nm)
    open(os.path.join(moddir, 'mod.rs'), 'w').write(modrs)

    # tests flatten (raw-string safe)
    test_marker = None
    for idx, l in enumerate(lines):
        if l.strip() == '#[cfg(test)]' and idx+1 < len(lines) and 'mod tests' in lines[idx+1]:
            test_marker = idx
            break
    has_tests = test_marker is not None
    if has_tests:
        # find module block end
        depth = 0; start = test_marker; end = None
        for k in range(test_marker, len(lines)):
            for ch in lines[k]:
                if ch == '{': depth += 1
                elif ch == '}': depth -= 1
            if depth == 0 and k > test_marker:
                end = k; break
        inner = lines[start+2:end]
        # raw-string-safe deindent
        out = []; in_raw = False; delim = None
        for l in inner:
            if not in_raw:
                m = re.search(r'r(#*)"', l)
                opened = False
                if m:
                    d = '#'*len(m.group(1)); delim = '"'+d
                    rest = l[m.end():]
                    if delim not in rest:
                        in_raw = True; opened = True
                out.append(l[4:] if l.startswith('    ') else l)
                if opened and delim and delim in (l[m.end():] if m else ''):
                    in_raw = False
            else:
                out.append(l)  # verbatim inside raw string
                if delim and delim in l:
                    in_raw = False
        flat = '\n'.join(out).strip('\n')
        # de-duplicate any repeated `use super::*;` the original test mod had.
        seen_glob = False
        dedup = []
        for ln in flat.split('\n'):
            if ln.strip() == 'use super::*;':
                if seen_glob:
                    continue
                seen_glob = True
            dedup.append(ln)
        flat = '\n'.join(dedup).strip('\n')
        # tests relied on the parent module's imports, so include the use-block.
        tuse = use_block + '\n\n' if use_block else ''
        if 'use super::*;' not in flat:
            flat = 'use super::*;\n\n' + flat
        full_tests = HDR + tuse + flat + '\n'
        full_tests = prune_unused_uses(full_tests)

        open(os.path.join(moddir, 'tests.rs'), 'w').write(full_tests)

    print('wrote', len(mods), 'item files (+mod.rs%s) in %s/' % (', tests.rs' if has_tests else '', moddir))
    print('files:', ', '.join(mods))

if __name__ == '__main__':
    main()

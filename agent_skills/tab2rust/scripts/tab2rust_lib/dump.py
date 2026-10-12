"""dump: compiled values of arrays, as JSON or one value per line."""
import json
import sys

from . import gdbrun, records


def flatten(d, v, out):
    """Scalar values in memory order. Floats as bit patterns, strings as JSON text."""
    k = d['kind']
    if k == 'array':
        for x in v:
            flatten(d['of'], x, out)
    elif k == 'struct':
        for f, x in zip(d['fields'], v):
            flatten(f['type'], x, out)
    elif k == 'float':
        out.append(f'float:{v["bits"]:#x}')
    elif k in ('str', 'ptr', 'fnptr'):
        if v is None:
            out.append('null')
        elif 's' in v:
            out.append('str:' + json.dumps(bytes.fromhex(v['s']).decode('latin-1')))
        else:
            out.append(f'ptr:{v.get("sym")}+{v.get("off")}')
    else:
        out.append(str(int(v)))


def run(a):
    parts = []
    for r in records.dump(a.binary, [gdbrun.parse_key(k) for k in a.symbol]):
        if 'error' in r:
            choices = f'. Choices: {", ".join(r["choices"])}' if r.get('choices') else ''
            print(f'ERROR {r["key"]}: {r["error"]}{choices}', file=sys.stderr)
        elif 'value' not in r:
            why = f'on the stack: {r["copy_error"]}' if r['storage'] == 'stack' else 'unsupported type: ' + ', '.join(r['unsupported'])
            print(f'ERROR {r["key"]}: no values ({why})', file=sys.stderr)
        elif a.flat:
            lines = []
            flatten(r['desc'], r['value'], lines)
            parts.append('\n'.join(lines))
        else:
            parts.append(json.dumps({k: r[k] for k in ('key', 'name', 'func', 'file', 'line', 'ctype', 'leaves',
                                                       'desc', 'value')}))
    text = '\n'.join(parts) + '\n'
    if a.out:
        with open(a.out, 'w') as f:
            f.write(text)
        print(f'wrote {a.out}')
    else:
        sys.stdout.write(text)

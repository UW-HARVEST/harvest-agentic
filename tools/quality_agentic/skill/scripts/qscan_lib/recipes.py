"""Rules that suggest recipes (see ../../RECIPES.md) for a function from its measured facts.

The rules only point at candidates. The recipe text in RECIPES.md gives the conditions that the
agent must check in the code before it applies a recipe.
"""

TITLES = {
    'R1': 'remove an unneeded unsafe block',
    'R2': 'remove `unsafe` from a function that has no unsafe operation',
    'R3': 'raw pointer parameter -> reference',
    'R4': 'pointer + length -> slice',
    'R5': 'C memory calls -> slice methods',
    'R6': 'local pointer walk -> index',
    'R7': 'static mut -> atomic, const or plain static',
    'R8': 'mem::zeroed -> a safe constructor',
    'R9': 'fix a clippy lint by hand',
    'R10': 'document the remaining unsafe (# Safety, // SAFETY:)',
    'R11': 'cast cleanup (as -> from / cast)',
    'R12': 'make a pseudo-safe function honest',
}


def suggest(row):
    """Recipe codes for one function row (a dict of functions.tsv fields), most valuable first."""
    out = []
    ops = row['ops']
    exported = row['vis'] == 'exported'
    trait_impl = row.get('trait_impl') == 'y'
    if row['pseudo_safe'] == 'y':
        out.append('R12')
    if row['unused_unsafe'] > 0:
        out.append('R1')
    if row['unsafe_fn'] == 'y' and ops == 0 and not trait_impl:
        out.append('R2')
    raw = row['raw_params'] > 0
    if raw and not trait_impl:
        # Per pointer variable: R3 for a parameter that the body only dereferences or passes on,
        # R4 when a parameter also walks a buffer.
        if row.get('r3_vars', '-') != '-':
            out.append('R3')
        if row.get('blocked_vars', '-') != '-' and (row['ptr_arith'] >= 1 or row['c_memory'] >= 1):
            out.append('R4')
        if 'R3' not in out and 'R4' not in out and row['deref'] >= 2:
            out.append('R4' if row['ptr_arith'] >= 1 or row['c_memory'] >= 1 else 'R3')
    if row['c_memory'] >= 1 and 'R4' not in out:
        out.append('R5')
    if row['ptr_arith'] >= 3 and 'R4' not in out:
        out.append('R6')
    if row['static_mut'] >= 1:
        out.append('R7')
    if row['zeroed'] >= 1:
        out.append('R8')
    if row['lints'] >= 1:
        out.append('R9')
    if row['cast_lints'] >= 1:
        out.append('R11')
    if ops > 0 or row['unsafe_fn'] == 'y':
        out.append('R10')
    if exported:
        # Exported: the C signature stays; the recipe works inside the body.
        out = [r + 'x' if r in ('R3', 'R4') else r for r in out]
    elif row['callers_elsewhere'] != '-':
        # Called from files that this unit does not own: R4 keeps a shim with the old signature.
        # R3 uses its in-body form, which keeps the signature.
        out = [r + 's' if r == 'R4' else r for r in out]
    return out


def score(row):
    """Ranking weight of a function in a worklist."""
    return (row['ops'] + row['lints'] + 0.3 * row['extra_lints'] + 0.5 * row['unused_unsafe']
            + (5 if row['unsafe_fn'] == 'y' and row['ops'] == 0 else 0)
            + (5 if row['pseudo_safe'] == 'y' else 0))

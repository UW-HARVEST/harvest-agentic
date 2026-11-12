#!/usr/bin/env python3
"""The Cargo feature names of the build options of a C project, and a check of Cargo.toml.

    cargo_features.py names [--option NAME=V1,V2,...] [--bool NAME]
    cargo_features.py check [--option NAME=V1,V2,...] [--bool NAME] [Cargo.toml]

The options come from inputs/c_src/configuration.json when it exists, otherwise from --option and
--bool. See ../SKILL.md.
"""
import argparse
import json
import sys
from pathlib import Path

CONFIG = Path('inputs/c_src/configuration.json')


def feature(name, value):
    """The feature of option `name` set to `value`. A boolean option has one feature for both values."""
    return name if isinstance(value, bool) else f'{name}_{value}'


def option_features(values_by_option):
    """The features of every value of every option, in order, without repeats."""
    return list(dict.fromkeys(feature(n, v) for n, values in values_by_option.items() for v in values))


def read_options(args):
    """Option name -> its values. None when there is no configuration file and no option."""
    if CONFIG.is_file():
        if args.option or args.bool:
            sys.exit(f'{CONFIG} exists: the options are its configurable_variables. Give no --option or --bool.')
        variables = json.loads(CONFIG.read_text()).get('configurable_variables')
        if not isinstance(variables, dict):
            sys.exit(f'{CONFIG} has no configurable_variables object')
        return variables
    if not (args.option or args.bool):
        return None
    options = {}
    for spec in args.option:
        name, sep, values = spec.partition('=')
        values = [v.strip() for v in values.split(',') if v.strip()]
        if not sep or not values:
            sys.exit(f'--option {spec}: write NAME=V1,V2,...')
        options[name.strip()] = values
    for name in args.bool:
        options[name] = [True, False]
    return options


def names(options):
    if not options:
        print('No build options: the crate needs no option features.')
        return 0
    print('[features]')
    print('# default = [...]')
    for name, values in options.items():
        print(f'# {name}: ' + ', '.join(json.dumps(v) for v in values))
        for f in option_features({name: values}):
            print(f'{f} = []')
    return 0


def check(options, manifest):
    import tomllib  # Python 3.11+; only check needs it

    with open(manifest, 'rb') as file:
        declared = tomllib.load(file).get('features', {})
    by_lower = {d.lower(): d for d in declared}
    wanted = option_features(options)
    problems = []
    for f in wanted:
        if f not in declared:
            near = by_lower.get(f.lower())
            problems.append(f'missing feature {f}' + (f' (Cargo.toml has {near})' if near else ''))
    for problem in problems:
        print(problem)
    if problems:
        print(f'{manifest}: {len(problems)} problem(s). Rename or add the features in [features], '
              'in default and in every cfg(feature = "...").')
        return 1
    print(f'OK: {manifest} declares all {len(wanted)} option features.')
    return 0


def main():
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument('--option', action='append', default=[], metavar='NAME=V1,V2,...',
                        help='a string or integer option and its values')
    common.add_argument('--bool', action='append', default=[], metavar='NAME', help='a boolean option')
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('names', parents=[common], help='print the option features as [features] lines')
    check_parser = commands.add_parser('check', parents=[common], help='check that Cargo.toml declares them')
    check_parser.add_argument('manifest', nargs='?', default='Cargo.toml', help='default: Cargo.toml')
    args = parser.parse_args()
    options = read_options(args)
    if options is None:
        print(f'No {CONFIG} and no --option or --bool. See SKILL.md for the options of the CMake files. '
              'A project without build options needs no option features.')
        return 0
    return names(options) if args.command == 'names' else check(options, args.manifest)


if __name__ == '__main__':
    sys.exit(main())

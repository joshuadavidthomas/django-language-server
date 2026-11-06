"""Ask real Django what DJLS should know about a project.

The `oracle` binary in djls-testing runs this script with the project's own
interpreter and compares its answers with DJLS's Static Extraction. It reads a
JSON request on stdin and writes one JSON object on stdout.

Request:
    settings_module   Django settings module to set up.
    sys_path          Directories to put first on sys.path (project and source roots).
    library_modules   {load_name: module} the request wants checked; they are
                      imported one by one so a broken library cannot hide others.
    template_names    Extra Template Names to resolve (DJLS's own inventory).
    include_django    Probe libraries whose module starts with `django.` too.

Nothing here reads DJLS facts: the answers come from importing and compiling.
This file must stay importable by every interpreter in the corpus, including
Python 3.6, so it avoids newer syntax.
"""

# Python 3.6 has neither future annotations nor zip(strict=...). Keep one script for the corpus.
# ruff: noqa: I002, UP031, B905

import inspect
import json
import os
import sys
import traceback

# Arguments tried after each tag name. The list mixes the shapes Django tag
# helpers and hand-written compile functions commonly accept.
ARGUMENT_CANDIDATES = [
    "",
    "a",
    "a b",
    "a b c",
    "a b c d",
    '"x"',
    "a as b",
    '"x" as y',
    "a=b",
    "a b=c",
    "a with b=c",
    "a for b",
    "a in b",
    "a by b",
]

MAX_BLOCK_DEPTH = 4
MAX_TEMPLATE_NAMES = 5000


class Unclosed(BaseException):
    """Raised instead of Django's unclosed-block error to capture what it waited for.

    BaseException so compile functions with `except Exception` cannot swallow it.
    """

    def __init__(self, parse_until):
        BaseException.__init__(self)
        self.parse_until = list(parse_until)


def main():
    request = json.load(sys.stdin)
    # Project code may print while importing; keep stdout for the answer alone.
    global ANSWER_STREAM
    ANSWER_STREAM = sys.stdout
    sys.stdout = sys.stderr
    for path in reversed(request.get("sys_path", [])):
        if path not in sys.path:
            sys.path.insert(0, path)
    os.environ["DJANGO_SETTINGS_MODULE"] = request["settings_module"]
    # Many apps skip database work in AppConfig.ready() under `check`; run as that command.
    sys.argv = ["manage.py", "check"]

    result = {"setup_error": None}
    try:
        import django

        result["django_version"] = django.get_version()
        django.setup()
    except (Exception, SystemExit):
        result["setup_error"] = traceback.format_exc(limit=8)
        emit(result)
        return

    try:
        engine, libraries = build_engine(
            request.get("library_modules") or {}, result, request.get("backend")
        )
    except Exception:
        result["setup_error"] = "template engine: " + traceback.format_exc(limit=8)
        emit(result)
        return

    replay = request.get("replay_templates")
    if replay is not None:
        result["native_probes"] = {
            source: probe_entry(source, *compile_source(engine, source))
            for source in replay
        }
        emit(result)
        return

    from django.template.base import Parser

    original = Parser.unclosed_block_tag
    install_parser_hooks()
    include_django = request.get("include_django", False)
    try:
        result["libraries"] = collect_libraries(engine, libraries, include_django)
    finally:
        Parser.unclosed_block_tag = original
    result["templates"] = collect_template_resolution(
        engine, request.get("template_names") or []
    )
    result["python_version"] = sys.version.split()[0]
    emit(result)


ANSWER_STREAM = sys.stdout


def emit(result):
    json.dump(result, ANSWER_STREAM, indent=1, sort_keys=True)
    ANSWER_STREAM.write("\n")


def build_engine(requested, result, backend_name=None):
    """Return the default Django engine, adding requested libraries that import.

    A library that fails to import is recorded, not fatal, so one broken module
    does not hide the facts of the rest.
    """
    from django.template import Engine
    from django.template import engines
    from django.template.backends.django import DjangoTemplates
    from django.template.library import import_library

    library_errors = {}
    valid = {}
    for load_name, module in sorted(requested.items()):
        try:
            import_library(module)
        except Exception as error:
            library_errors[module] = classify_import_error(error)
        else:
            valid[load_name] = module

    backends = [
        backend for backend in engines.all() if isinstance(backend, DjangoTemplates)
    ]
    result["backend_names"] = [backend.name for backend in backends]
    if backend_name is None and len(backends) != 1:
        raise RuntimeError(
            "Select a backend with --backend: %r" % result["backend_names"]
        )
    selected = next(
        (
            backend
            for backend in backends
            if backend_name is None or backend.name == backend_name
        ),
        None,
    )
    if selected is None:
        raise RuntimeError("No Django template backend %r" % backend_name)
    result["backend"] = selected.name
    default = selected.engine

    libraries = dict(default.libraries)
    libraries.update(valid)
    # Copy the project's engine options; the accepted keywords differ across Django releases.
    accepted = inspect.signature(Engine.__init__).parameters
    options = {
        name: getattr(default, name)
        for name in (
            "dirs",
            "context_processors",
            "loaders",
            "string_if_invalid",
            "file_charset",
            "autoescape",
        )
        if name in accepted and hasattr(default, name)
    }
    engine = (
        default
        if not requested
        else Engine(
            debug=False,
            libraries=libraries,
            builtins=[
                module
                for module in default.builtins
                if module not in Engine.default_builtins
            ],
            **options,
        )
    )
    result["library_errors"] = library_errors
    result["builtins"] = list(engine.builtins)
    result["load_names"] = dict(sorted(engine.libraries.items()))
    return engine, libraries


def classify_import_error(error):
    message = str(error)
    # `import_library` reports both a failed import and a module without a
    # module-level `register`; only the second is a fact about the source.
    if "does not have a variable named" in message:
        return {"kind": "not_a_library", "message": message}
    return {"kind": "import_failed", "message": message.splitlines()[0][:500]}


def collect_libraries(engine, libraries, include_django):
    collected = []
    seen = set()
    for module, library in zip(engine.builtins, engine.template_builtins):
        if module in seen:
            continue
        seen.add(module)
        collected.append(library_facts(engine, library, module, None, include_django))
    for load_name in sorted(engine.libraries):
        module = engine.libraries[load_name]
        library = engine.template_libraries[load_name]
        collected.append(
            library_facts(engine, library, module, load_name, include_django)
        )
    return collected


def library_facts(engine, library, module, load_name, include_django):
    probe = include_django or not module.startswith("django.")
    facts = {
        "module": module,
        "load_name": load_name,
        "probed": probe,
        "tags": {},
        "filters": {},
    }
    prefix = "{%% load %s %%}" % load_name if load_name else ""
    for name in sorted(library.tags):
        func = library.tags[name]
        tag = {"kind": tag_kind(func), "defined_in": getattr(func, "__module__", None)}
        if probe:
            tag.update(probe_tag(engine, prefix, name, func))
        facts["tags"][name] = tag
    for name in sorted(library.filters):
        func = library.filters[name]
        entry = {"defined_in": getattr(func, "__module__", None)}
        if probe:
            entry.update(probe_filter(engine, prefix, name, func))
        facts["filters"][name] = entry
    return facts


def tag_kind(func):
    qualname = getattr(func, "__qualname__", "") or ""
    for helper in ("simple_block_tag", "simple_tag", "inclusion_tag"):
        if "." + helper + "." in qualname:
            return helper
    return "compile_function"


def install_parser_hooks():
    from django.template.base import Parser

    def unclosed_block_tag(self, parse_until):
        # Parser.parse matches only a token's first word, so `endfoo name` waits for `endfoo`.
        stops = []
        for stop in parse_until:
            words = str(stop).split()
            if words and words[0] not in stops:
                stops.append(words[0])
        raise Unclosed(stops)

    Parser.unclosed_block_tag = unclosed_block_tag


def compile_source(engine, source):
    """Compile with Django: ("compiled", None), ("unclosed", stops) or ("rejected", message)."""
    from django.template import TemplateSyntaxError

    try:
        engine.from_string(source)
    except Unclosed as unclosed:
        return "unclosed", unclosed.parse_until
    except TemplateSyntaxError as error:
        return "rejected", str(error)[:300]
    except Exception as error:
        return "unchecked", "%s: %s" % (type(error).__name__, str(error)[:300])
    return "compiled", None


def opening(name, arguments):
    return "{%% %s%s %%}" % (name, " " + arguments if arguments else "")


def probe_tag(engine, prefix, name, func):
    """Find invocations Django accepts and the block structure they open.

    Each candidate argument list is compiled alone. A candidate that reaches
    `Parser.parse` at end of input opens a block; the stop tags Django waited
    for are then fed back one at a time to tell closers from intermediates.
    """
    probes = []
    block = None
    # The fewest Tag Bits of any accepted invocation; DJLS must not require more.
    min_bits = None
    candidates = list(ARGUMENT_CANDIDATES)
    try:
        signature = inspect.signature(inspect.unwrap(func))
        positional = [
            p
            for p in signature.parameters.values()
            if p.kind in (p.POSITIONAL_ONLY, p.POSITIONAL_OR_KEYWORD)
        ]
        keywords = [
            p.name
            for p in signature.parameters.values()
            if p.kind == p.KEYWORD_ONLY and p.default == p.empty
        ]
        for count in range(len(positional) + 2):
            candidates.append(
                " ".join(["a"] * count + [key + "=a" for key in keywords])
            )
    except (TypeError, ValueError):
        pass
    for arguments in dict.fromkeys(candidates):
        source = prefix + opening(name, arguments)
        outcome, detail = compile_source(engine, source)
        probes.append(probe_entry(source, outcome, detail))
        compiled = outcome == "compiled"
        if outcome == "unclosed":
            if block is None:
                block = explore_block(engine, source, detail)
                block["opening_arguments"] = arguments
                probes.extend(block.pop("probes"))
            elif block["stop_tags"] != detail:
                block["unexplored"]["opening: " + arguments] = (
                    "alternative stop tags: " + ", ".join(detail)
                )
            if block["end_tags"]:
                closed = source + "{%% %s %%}" % block["end_tags"][0]
                closed_outcome = compile_source(engine, closed)
                probes.append(probe_entry(closed, *closed_outcome))
                compiled = closed_outcome[0] == "compiled"
        if compiled:
            from django.utils.text import smart_split

            bits = len(list(smart_split(arguments)))
            min_bits = bits if min_bits is None else min(min_bits, bits)
    result = {"probes": dedupe(probes), "min_compiled_bits": min_bits}
    if block is not None:
        result["block"] = block
    return result


def probe_entry(template, outcome, detail):
    entry = {
        "template": template,
        "django": outcome if outcome != "unclosed" else "rejected",
    }
    if outcome in ("rejected", "unchecked"):
        entry["error"] = detail
    elif outcome == "unclosed":
        entry["error"] = "unclosed block, waiting for: " + ", ".join(detail)
    return entry


def dedupe(probes):
    seen = set()
    unique = []
    for probe in probes:
        if probe["template"] in seen:
            continue
        seen.add(probe["template"])
        unique.append(probe)
    return unique


MAX_BLOCK_COMPILES = 64


def explore_block(engine, opener, first_stops):
    """Classify each stop tag as a closer or an intermediate by feeding it back.

    A stop that lets the template compile is a closer. A stop after which Django
    waits again is an intermediate, and its new stop tags are explored in turn.
    """
    end_tags = []
    intermediates = []
    unexplored = {}
    probes = []
    frontier = [(opener, list(first_stops), 0)]
    compiles = 0
    while frontier and compiles < MAX_BLOCK_COMPILES:
        source, stops, depth = frontier.pop(0)
        for stop in stops:
            if not stop or compiles >= MAX_BLOCK_COMPILES:
                continue
            candidate = source + "{%% %s %%}" % stop
            outcome, detail = compile_source(engine, candidate)
            compiles += 1
            probes.append(probe_entry(candidate, outcome, detail))
            if outcome == "compiled":
                if stop not in end_tags:
                    end_tags.append(stop)
            elif outcome == "unclosed":
                if stop not in intermediates:
                    intermediates.append(stop)
                if depth + 1 < MAX_BLOCK_DEPTH:
                    frontier.append((candidate, detail, depth + 1))
                else:
                    unexplored["depth-limit: " + stop] = (
                        "remaining stop tags: " + ", ".join(detail)
                    )
            else:
                unexplored.setdefault(stop, detail)
    intermediates = [tag for tag in intermediates if tag not in end_tags]
    for tag in end_tags + intermediates:
        unexplored.pop(tag, None)
    if frontier or compiles >= MAX_BLOCK_COMPILES:
        unexplored["compile-budget"] = "block exploration reached its compile limit"
    return {
        "stop_tags": list(first_stops),
        "end_tags": end_tags,
        "intermediates": intermediates,
        "unexplored": unexplored,
        "probes": probes,
    }


def probe_filter(engine, prefix, name, func):
    from django.template import TemplateSyntaxError
    from django.template.base import FilterExpression

    accepts = []
    for provided in ([], ["arg"]):
        try:
            FilterExpression.args_check(name, func, provided)
        except TemplateSyntaxError:
            accepts.append(False)
        except Exception:
            return {"arity": None, "probes": []}
        else:
            accepts.append(True)
    arity = {
        (True, False): "no_argument",
        (False, True): "required_argument",
        (True, True): "optional_argument",
        (False, False): "no_valid_invocation",
    }[tuple(accepts)]
    probes = []
    for suffix in ("", ":a"):
        source = prefix + "{{ v|%s%s }}" % (name, suffix)
        outcome, detail = compile_source(engine, source)
        probes.append(probe_entry(source, outcome, detail))
    return {"arity": arity, "probes": probes}


def find_origin(engine, name):
    """Use the loader's public operation, including custom get_template overrides."""
    from django.template import Template
    from django.template import TemplateDoesNotExist
    from django.template import TemplateSyntaxError

    try:
        origin = engine.get_template(name).origin.name
    except TemplateDoesNotExist:
        return None
    except TemplateSyntaxError as error:
        # Django selected a source before compiling it. Read that actual Template,
        # not a guessed filesystem candidate or a replacement loader operation.
        # Match the requested name so a nested compilation cannot supply its origin.
        trace = error.__traceback__
        while trace is not None:
            template = trace.tb_frame.f_locals.get("self")
            if isinstance(template, Template) and template.origin.template_name == name:
                origin = template.origin.name
                break
            trace = trace.tb_next
        else:
            raise
    return os.path.realpath(origin) if os.path.isabs(origin) else origin


def collect_template_resolution(engine, extra_names):
    """Resolve every Template Name the loaders can see, plus the names DJLS knows."""
    names = set(extra_names)
    names.add("__djls_oracle_missing_template__.html")
    for loader in engine.template_loaders:
        for inner in getattr(loader, "loaders", [loader]):
            get_dirs = getattr(inner, "get_dirs", None)
            if get_dirs is None:
                continue
            for directory in get_dirs():
                directory = str(directory)
                for root, dirnames, filenames in os.walk(directory):
                    dirnames[:] = [d for d in dirnames if not d.startswith(".")]
                    for filename in filenames:
                        path = os.path.join(root, filename)
                        names.add(os.path.relpath(path, directory).replace(os.sep, "/"))
    resolved = {}
    truncated = len(names) > MAX_TEMPLATE_NAMES
    for name in sorted(names)[:MAX_TEMPLATE_NAMES]:
        try:
            resolved[name] = find_origin(engine, name)
        except Exception as error:
            resolved[name] = {
                "error": "%s: %s" % (type(error).__name__, str(error)[:200])
            }
    return {"resolved": resolved, "truncated": truncated, "cwd": os.getcwd()}


if __name__ == "__main__":
    main()

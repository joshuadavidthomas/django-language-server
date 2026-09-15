from pathlib import Path as P
import os as operating_system
from os.path import join as path_join, dirname as path_dirname

stringify = str
MODULE_FILE = __file__
ROOT = P(__file__).parent
RESOLVED = P(__file__).resolve()
FIRST_PARENT = P(__file__).resolve().parents[0]
SECOND_PARENT = P(__file__).resolve().parents[1]
NORMALIZED = P(__file__).parent.joinpath("..").resolve()
TEMPLATES_DIR = operating_system.path.join(ROOT, "templates")
STATIC_DIR = path_join(ROOT, "static")
PARENT = path_dirname(TEMPLATES_DIR)
EMPTY_PARENT = path_dirname("")
TRAILING_PARENT = path_dirname("/project/")
ROOT_PARENT = path_dirname("/")
STATIC_TEXT = stringify(STATIC_DIR)
RELATIVE_PATH = P("relative")
INVALID_PARENTS = P(__file__).parents[2]
INVALID_METHOD = TEMPLATES_DIR.parent
INVALID_DIVISION = TEMPLATES_DIR / "nested"

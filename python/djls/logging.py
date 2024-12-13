from __future__ import annotations

import logging
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass
class LogConfig:
    log_file: Path | str = "/tmp/djls_debug.log"
    log_level: int = logging.DEBUG
    console_level: int = logging.DEBUG
    file_level: int = logging.DEBUG
    format: str = "%(asctime)s - %(name)s - %(levelname)s - %(message)s"


def configure_logging(config: LogConfig | None = None) -> logging.Logger:
    if config is None:
        config = LogConfig()

    logger = logging.getLogger("djls")
    logger.setLevel(config.log_level)

    # Clear any existing handlers
    logger.handlers.clear()

    # File handler
    fh = logging.FileHandler(config.log_file)
    fh.setLevel(config.file_level)

    # Console handler
    ch = logging.StreamHandler(sys.stderr)
    ch.setLevel(config.console_level)

    # Formatter
    formatter = logging.Formatter(config.format)
    fh.setFormatter(formatter)
    ch.setFormatter(formatter)

    logger.addHandler(fh)
    logger.addHandler(ch)

    return logger

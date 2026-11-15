from __future__ import annotations

from django.template import Origin
from django.template import Template
from django.template.loaders.filesystem import Loader as FilesystemLoader


class Loader(FilesystemLoader):
    def get_template(self, template_name, skip=None):
        if template_name == "shadow.html":
            return Template(
                "runtime override",
                origin=Origin("virtual://shadow.html"),
                engine=self.engine,
            )
        return super().get_template(template_name, skip=skip)

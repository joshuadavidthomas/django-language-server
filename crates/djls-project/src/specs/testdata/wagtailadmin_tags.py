# Vendored unit-test fixture.
# Corpus: wagtail-7.3/wagtail/admin/templatetags/wagtailadmin_tags.py
# Keep snippets minimal: live corpus drift is covered by crates/djls-project/tests/corpus*.rs.

from django import template

register = template.Library()

def intcomma(value):
    return value

register.filter("intcomma", intcomma)

class DialogNode:
    @classmethod
    def handle(cls, parser, token):
        return cls()

register.tag("dialog", DialogNode.handle)

# Vendored unit-test fixture.
# Corpus: wagtail-7.3/wagtail/admin/templatetags/wagtailadmin_tags.py
# Keep snippets minimal: live corpus drift is covered by crates/djls-project/tests/corpus*.rs.

from django import template

register = template.Library()

register.filter("intcomma", intcomma)

class DialogNode(BlockInclusionNode):
    template = "wagtailadmin/shared/dialog/dialog.html"

    def get_context_data(self, parent_context):
        context = super().get_context_data(parent_context)

        if "title" not in context:
            raise TypeError("You must supply a title")
        if "id" not in context:
            raise TypeError("You must supply an id")

        # Used for determining which icon the message will use
        message_icon_name = {
            "info": "info-circle",
            "warning": "warning",
            "critical": "warning",
            "success": "circle-check",
        }

        message_status = context.get("message_status")

        # If there is a message status then determine which icon to use.
        if message_status:
            context["message_icon_name"] = message_icon_name[message_status]

        return context

register.tag("dialog", DialogNode.handle)

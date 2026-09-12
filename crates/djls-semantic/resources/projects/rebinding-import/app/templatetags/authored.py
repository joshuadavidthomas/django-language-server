from django import template
from app.implementation import imported
register = template.Library()
register.simple_tag(imported, name='loaded_imported')

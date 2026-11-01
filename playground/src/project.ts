import { DJANGO_TEMPLATE } from "./django-template";
import type { DjlsSettings, Files } from "./worker/protocol";

export const SETTINGS: DjlsSettings = { django_settings_module: "mysite.settings" };

const STORAGE_KEY = "djls-playground:files";

export const EXAMPLE_FILES: Files = {
  "mysite/__init__.py": "",
  "mysite/settings.py": `from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent.parent

INSTALLED_APPS = [
    "django.contrib.auth",
    "django.contrib.contenttypes",
    "blog",
]

TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [BASE_DIR / "templates"],
        "APP_DIRS": True,
    },
]
`,
  "blog/__init__.py": "",
  "blog/models.py": `from django.db import models


class Post(models.Model):
    title = models.CharField(max_length=200)
    body = models.TextField()
    published = models.DateTimeField()
`,
  "blog/templatetags/__init__.py": "",
  "blog/templatetags/blog_tags.py": `from django import template

register = template.Library()


@register.simple_tag
def greet(name, punctuation="!"):
    return f"Hello {name}{punctuation}"


@register.filter
def shout(value):
    return value.upper()


@register.inclusion_tag("blog/includes/post_card.html")
def post_card(post):
    return {"post": post}
`,
  "templates/base.html": `<!doctype html>
<html lang="en">
<head>
  <title>{% block title %}My site{% endblock %}</title>
</head>
<body>
  {% block content %}{% endblock %}
</body>
</html>
`,
  "blog/templates/blog/includes/post_card.html": `<article>
  <h2>{{ post.title|title }}</h2>
  <time datetime="{{ post.published|date:'c' }}">{{ post.published|timesince }} ago</time>
</article>
`,
  "blog/templates/blog/post_list.html": `{% extends "base.html" %}
{% load blog_tags %}

{% block title %}Posts{% endblock %}

{% block content %}
  <h1>{% greet user.username %}</h1>
  {# greet needs a name #}
  {% greet %}

  {% for post in posts %}
    {% post_card post %}
    <p class="{% if forloop.first %}lead{% endif %}">{{ post.body|truncatewords:30 }}</p>
  {% empty %}
    <p>{{ "no posts yet"|shout:"!" }}</p>
  {% endfor %}

  {% bogus_tag %}
{% endblock %}
`,
};

export function loadFiles(): Files {
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved) {
    try {
      return JSON.parse(saved) as Files;
    } catch {
      localStorage.removeItem(STORAGE_KEY);
    }
  }
  return { ...EXAMPLE_FILES };
}

export function saveFiles(files: Files) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(files));
}

export function clearSavedFiles() {
  localStorage.removeItem(STORAGE_KEY);
}

/** Template files get Django highlighting; everything else is Python or plain text. */
export function languageFor(path: string): string {
  if (/\.(html?|djhtml|txt|xml)$/.test(path)) return DJANGO_TEMPLATE;
  if (path.endsWith(".py")) return "python";
  return "plaintext";
}

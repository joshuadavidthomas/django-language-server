from django.db import models
from django.contrib.contenttypes.fields import GenericForeignKey

class TaggedItem(models.Model):
    target = models.ForeignKey("Missing", on_delete=models.CASCADE)
    content_object = GenericForeignKey()

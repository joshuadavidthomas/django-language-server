from django.db import models
from django.contrib.contenttypes.fields import GenericForeignKey
import accounts.models as account_models

class Post(models.Model):
    author = models.ForeignKey("accounts.User", on_delete=models.CASCADE)
    editor = models.ForeignKey(account_models.User, on_delete=models.CASCADE)
    parent = models.ForeignKey("self", on_delete=models.CASCADE)
    tags = models.ManyToManyField("Tag")
    content_object = GenericForeignKey()

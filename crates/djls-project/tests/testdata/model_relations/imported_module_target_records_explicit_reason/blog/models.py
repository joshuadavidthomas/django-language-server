from django.db import models
import accounts.models

class Post(models.Model):
    author = models.ForeignKey(accounts.models, on_delete=models.CASCADE)

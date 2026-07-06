from django.db import models
from accounts.models import Profile

class Post(models.Model):
    author = models.ForeignKey(Profile, on_delete=models.CASCADE)

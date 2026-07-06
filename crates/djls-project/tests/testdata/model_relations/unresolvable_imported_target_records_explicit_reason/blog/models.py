from django.db import models
from missing.models import User

class Post(models.Model):
    author = models.ForeignKey(User, on_delete=models.CASCADE)

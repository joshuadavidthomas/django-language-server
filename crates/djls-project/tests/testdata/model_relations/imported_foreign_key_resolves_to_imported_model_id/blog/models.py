from django.db import models
from accounts.models import User

class User(models.Model):
    pass

class Post(models.Model):
    author = models.ForeignKey(User, on_delete=models.CASCADE)

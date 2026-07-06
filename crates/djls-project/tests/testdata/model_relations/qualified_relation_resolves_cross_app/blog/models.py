from django.db import models

class User(models.Model):
    pass

class Post(models.Model):
    author = models.ForeignKey("accounts.User", on_delete=models.CASCADE)

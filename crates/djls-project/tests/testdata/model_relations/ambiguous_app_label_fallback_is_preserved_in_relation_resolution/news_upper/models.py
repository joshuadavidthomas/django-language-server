from django.db import models

class Post(models.Model):
    author = models.ForeignKey("User", on_delete=models.CASCADE)

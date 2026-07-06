from django.db import models

class User(models.Model):
    pass

class Profile(models.Model):
    user = models.ForeignKey("User", on_delete=models.CASCADE)

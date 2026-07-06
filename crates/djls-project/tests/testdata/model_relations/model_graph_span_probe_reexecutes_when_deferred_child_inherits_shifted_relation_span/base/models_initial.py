from django.db import models

class User(models.Model):
    pass

class TimeStamped(models.Model):
    owner = models.ForeignKey("base.User", on_delete=models.CASCADE)

    class Meta:
        abstract = True

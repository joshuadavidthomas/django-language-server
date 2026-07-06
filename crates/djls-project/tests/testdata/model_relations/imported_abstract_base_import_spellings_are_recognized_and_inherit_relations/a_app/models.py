from django.db import models

class User(models.Model):
    pass

class TimeStamped(models.Model):
    owner = models.ForeignKey("a_app.User", on_delete=models.CASCADE)

    class Meta:
        abstract = True

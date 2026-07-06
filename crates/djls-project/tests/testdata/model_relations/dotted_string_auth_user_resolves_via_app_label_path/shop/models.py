from django.db import models

class User(models.Model):
    pass

class Order(models.Model):
    user = models.ForeignKey("auth.User", on_delete=models.CASCADE)

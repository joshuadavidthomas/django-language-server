from django.db import models

class Category(models.Model):
    parent = models.ForeignKey("self", on_delete=models.CASCADE)

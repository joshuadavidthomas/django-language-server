from django.db import models
import accounts.models as account_models

class User(models.Model):
    pass

class Post(models.Model):
    author = models.ForeignKey(account_models.User, on_delete=models.CASCADE)

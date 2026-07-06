from django.db import models

class BaseTarget(models.Model):
    pass

class AbstractBase(models.Model):
    base_owner = models.ForeignKey("base.BaseTarget", on_delete=models.CASCADE)

    class Meta:
        abstract = True

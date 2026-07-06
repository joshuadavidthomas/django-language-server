from django.db import models

class GrandTarget(models.Model):
    pass

class ParentTarget(models.Model):
    pass

class AbstractGrandparent(models.Model):
    grand_owner = models.ForeignKey("inheritance.GrandTarget", on_delete=models.CASCADE)

    class Meta:
        abstract = True

class AbstractParent(AbstractGrandparent):
    parent_owner = models.ForeignKey("inheritance.ParentTarget", on_delete=models.CASCADE)

    class Meta:
        abstract = True

class SameFileChild(AbstractParent):
    pass

class SameFileSibling(AbstractParent):
    pass

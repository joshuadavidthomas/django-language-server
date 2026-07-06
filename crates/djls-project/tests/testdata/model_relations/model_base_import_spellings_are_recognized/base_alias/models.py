from django.db.models import Model as Base
import django.db.models as m

class FromBase(Base):
    pass

class FromModule(m.Model):
    pass

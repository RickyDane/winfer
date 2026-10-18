from dataclasses import dataclass, field
from datetime import date

from app.slugs import make_slug


@dataclass
class Article:
    title: str
    published: date
    slug: str = field(init=False)

    def __post_init__(self):
        self.slug = make_slug(self.title)

    @property
    def url(self):
        return f"/blog/{self.published.year}/{self.slug}/"

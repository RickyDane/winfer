"""Create the articles table."""

SQL = """
CREATE TABLE articles (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    slug VARCHAR(60) NOT NULL UNIQUE,
    published DATE NOT NULL
);
"""

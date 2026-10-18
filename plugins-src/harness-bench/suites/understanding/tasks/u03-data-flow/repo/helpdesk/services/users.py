from ..db import users_repo
from ..sanitize import quote


def rename(user_id, display_name, email=None):
    users_repo.set_display_name(int(user_id), quote(display_name.strip()))
    if email:
        users_repo.set_email(int(user_id), email.strip())


def list_users(team):
    return users_repo.by_team(team)

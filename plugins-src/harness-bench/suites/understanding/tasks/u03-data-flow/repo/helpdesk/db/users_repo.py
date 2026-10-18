from .conn import execute


def set_display_name(user_id, quoted_name):
    # quoted_name is already a SQL literal (see sanitize.quote)
    return execute(f"UPDATE users SET display_name = {quoted_name} WHERE id = {user_id}")


def set_email(user_id, email):
    return execute("UPDATE users SET email = ? WHERE id = ?", (email, user_id))


def by_team(team):
    return execute("SELECT id, login FROM users WHERE team = ?", (team,))

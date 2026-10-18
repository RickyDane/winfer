from .conn import execute


def find(region, status, owner, text, order, limit):
    where, params = ["1=1"], []
    if region:
        where.append("region = ?")
        params.append(region)
    if status:
        where.append(f"status = '{status}'")
    if owner:
        where.append(f"owner_login LIKE '{owner}%'")
    if text:
        where.append("title LIKE ?")
        params.append(f"%{text}%")
    sql = f"SELECT id, title FROM tickets WHERE {' AND '.join(where)} ORDER BY {order} LIMIT {limit}"
    return execute(sql, params)


def insert(title, note, reporter):
    return execute("INSERT INTO tickets (title, note, reporter) VALUES (?, ?, ?)", (title, note, reporter))

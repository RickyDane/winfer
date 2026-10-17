python3 - <<'PY'
p = "contacts/cli.py"
s = open(p).read()
s = s.replace('a.add_argument("name")', 'a.add_argument("first")\n    a.add_argument("last")')
s = s.replace("repo.add_person(conn, args.name, args.email)", "repo.add_person(conn, args.first, args.last, args.email)")
s = s.replace("add NAME", "add FIRST LAST")
open(p, "w").write(s)
PY
python3 -m contacts.cli migrate >/dev/null

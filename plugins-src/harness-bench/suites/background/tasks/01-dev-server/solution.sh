python3 server.py > /tmp/oracle-srv.$$ 2>&1 &
pid=$!
until grep -q "ready: listening" /tmp/oracle-srv.$$; do sleep 0.2; done
url=$(sed -n 's/.*listening on //p' /tmp/oracle-srv.$$)
curl -s "$url/api/secret" > answer.txt
kill $pid; wait $pid 2>/dev/null; rm -f /tmp/oracle-srv.$$
exit 0

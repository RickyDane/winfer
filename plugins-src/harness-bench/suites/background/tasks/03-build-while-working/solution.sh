./build.sh > /tmp/oracle-build.$$ 2>&1 &
pid=$!
sed -i.bak 's/1.4.1"/1.5.0"/' version.py && rm version.py.bak
printf '# Changelog\n\n## 1.5.0\n- Faster startup.\n\n## 1.4.1\n- Fix crash on empty config.\n' > CHANGELOG.md
wait $pid
sed -n 's/.*BUILD ID: //p' /tmp/oracle-build.$$ > answer.txt; rm -f /tmp/oracle-build.$$
exit 0

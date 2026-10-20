#!/bin/sh
# Release build: slow (~25 s) and chatty. Prints the build id at the end.
echo "Building release…"
for i in $(seq 1 250); do echo "   Compiling module_$i v0.1.0"; sleep 0.1; done
id=$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')
echo "$id" > .build-id
echo "Finished release build. BUILD ID: $id"

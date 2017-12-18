#!/bin/bash

INPAR=2;
MIN_THREADS=1;
MAX_THREADS=$1;
MSGS=1000;
AMP=50000;
BIN_LOC="../target/release/executor-performance"
OUT_LOC="out.csv"
rm $OUT_LOC;
for i in $(seq $MIN_THREADS $MAX_THREADS); do
	echo "Run $i starting";
	$BIN_LOC -t $i -p $INPAR -m $MSGS -a $AMP -o $OUT_LOC --pre 100 --post 100 --skip-tpe;
	echo "Run $i finished";
done
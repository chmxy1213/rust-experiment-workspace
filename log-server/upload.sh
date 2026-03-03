#!/bin/bash

curl -X POST http://127.0.0.1:8001/api/upload \
  -F "agent_name=test-agent" \
  -F "ip=192.168.1.110" \
  -F "app=pcli" \
  -F "task-id=111-111" \
  -F "filename=pcli.log" \
  -F "file=@/Users/secvision/RustProject/rust-experiment-workspace/log-server/simple-2.log"

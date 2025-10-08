  DevEnv TUI Unfuck Guide

  Problem: DevEnv TUI crashes with clipboard/glippy errors, especially when trying to copy text (Ctrl+S).

  Unfuck Steps:
  # 1. Kill all processes
  pkill -9 process-compose
  pkill -9 -f devenv
  pkill -9 -f nix

  # 2. Remove socket files (KEY STEP!)
  rm -f /run/user/0/devenv-*/pc.sock
  rm -rf /run/user/0/devenv-*
  rm -rf /tmp/*devenv*

  # 3. Reset terminal
  reset

  # 4. Restart (headless mode recommended)
  cd /opt/hashpool
  export CDK_PATH=/opt/cdk/crates/cdk
  devenv shell
  just up --headless

  Quick Nuclear Option:
  pkill -9 process-compose && rm -rf /run/user/0/devenv-* && reset

  Key: Always use --headless mode on VPS to avoid clipboard crashes!

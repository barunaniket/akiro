# Akiro - Ultra-Fast Distributed Competitive Programming Sandbox
# Usage: just <recipe>

default:
    @just --list

# Build the Akiro Docker sandbox image
build:
    docker build -t akiro:latest .

# Run cargo type check locally
check:
    cargo check

# Run local cargo test suite
test:
    cargo test

# Start Akiro as a distributed worker node (joining leader Redis broker)
worker redis_url="redis://:ceef3470b081d9f851ea3acc65efc4a0fd61f65d3d426998f49f57e37a945e5f@host.docker.internal:6380" workers="8":
    docker rm -f akiro-worker 2>/dev/null || true
    docker run -d --name akiro-worker --privileged --restart unless-stopped \
        -e ENABLE_EMBEDDED_REDIS=false \
        akiro:latest --mode worker --redis {{redis_url}} --workers {{workers}}
    @echo "Akiro worker node started with {{workers}} workers connected to {{redis_url}} ✓"

# Open keepalive SSH tunnel to Azure leader VM on port 6380
tunnel vm_ip="20.219.186.217" key_path="C:\Users\Aniket Barun/Downloads/azure-judge-key.pem":
    ssh -i "{{key_path}}" -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=15 -o ServerAliveCountMax=6 -o GatewayPorts=yes -N -L 0.0.0.0:6380:127.0.0.1:6379 azureuser@{{vm_ip}}

# Check real-time cluster health and active worker count
health endpoint="https://20.219.186.217.nip.io/health" secret="ceef3470b081d9f851ea3acc65efc4a0fd61f65d3d426998f49f57e37a945e5f":
    curl -s -H "X-Judge-Secret: {{secret}}" {{endpoint}}
    @echo ""

# View live logs of the local worker container
logs:
    docker logs -f --tail 50 akiro-worker

# Stop and remove local worker container
stop:
    docker rm -f akiro-worker

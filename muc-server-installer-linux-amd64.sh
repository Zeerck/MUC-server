#!/bin/bash

DOWNLOAD_URL="https://github.com/Zeerck/MUC-server/releases/latest/download/MUC-server-linux-amd64.tar.gz"
APP_USER="muc-server"
INSTALL_DIR="/opt/muc-server"
CONFIG_DIR="/etc/muc-server"
DATA_DIR="/var/lib/muc-server"
SERVICE_FILE="/etc/systemd/system/muc-server.service"
ENV_FILE="$CONFIG_DIR/muc-server.env"

RED=$'\033[0;31m'
GREEN=$'\033[0;32m'
YELLOW=$'\033[1;33m'
CYAN=$'\033[0;36m'
NC=$'\033[0m'

print_message() {
    echo -e "${GREEN}[+]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[!]${NC} $1"
}

print_error() {
    echo -e "${RED}[-]${NC} $1"
}

grant_cert_access() {
    local file_path="$1"
    local real_path=$(readlink -f "$file_path")

    if ! command -v setfacl &> /dev/null; then
        print_error "'setfacl' package not found. Installing with: apt install acl..."
        apt install acl
    fi

    setfacl -m u:"$APP_USER":r "$real_path" 2>/dev/null || {
        print_error "Cannot grand rights for file: $real_path"
        exit 1
    }

    local dir_path=$(dirname "$real_path")
    while [ "$dir_path" != "/" ]; do
        setfacl -m u:"$APP_USER":x "$dir_path" 2>/dev/null || {
            print_error "Cannot grand rights for directory: $dir_path"
            exit 1
        }
        dir_path=$(dirname "$dir_path")
    done
}

if [ "$1" == "--remove" ]; then
    print_warning "Deleting MUC-server..."
    
    systemctl stop muc-server 2>/dev/null
    systemctl disable muc-server 2>/dev/null
    rm -f "$SERVICE_FILE"
    systemctl daemon-reload
    
    rm -rf "$INSTALL_DIR"
    
    printf "%s[?]%s Do you want do delete configs, database and settings? (y/n): " "$YELLOW" "$NC"
    read del_data
    if [[ "$del_data" =~ ^[Yy]$ ]]; then
        rm -rf "$CONFIG_DIR" "$DATA_DIR"
        userdel "$APP_USER" 2>/dev/null
        print_message "MUC-server deleted completly."
    else
        print_message "MUC-server deleted. Configs and setting saved in $DATA_DIR and $CONFIG_DIR"
    fi
    exit 0
fi

if [ "$EUID" -ne 0 ]; then
    print_error "Please run this script using `sudo bash muc-server-installer.sh`"
    exit 1
fi

print_message "Installing MUC-server..."

if id "$APP_USER" &>/dev/null; then
    print_warning "user $APP_USER already exist. Skipping creating new user."
else
    useradd -r -s /bin/false "$APP_USER"
    print_message "Created system user: $APP_USER."
fi

mkdir -p "$INSTALL_DIR" "$CONFIG_DIR" "$DATA_DIR"

echo -e "${CYAN}=== Server settings ===${NC}"

while true; do
    printf "%s[?]%s Enter path with .cert file of domain (TLS_CERT_PATH): " "$YELLOW" "$NC"
    read tls_cert
    if [ -f "$tls_cert" ]; then
        break
    else
        print_error "File .cert not found. Try again."
    fi
done

while true; do
    printf "%s[?]%s Enter path with .key file of domain (TLS_KEY_PATH): " "$YELLOW" "$NC"
    read tls_key
    if [ -f "$tls_key" ]; then
        break
    else
        print_error "File .key not found. Try again."
    fi
done

printf "%s[?]%s Server address:port [0.0.0.0:1990]: " "$YELLOW" "$NC"
read server_addr
server_addr=${server_addr:-"0.0.0.0:1990"}

printf "%s[?]%s Path where DB will placed [/var/lib/muc-server/database.sqlite]: " "$YELLOW" "$NC"
read db_path
db_path=${db_path:-"/var/lib/muc-server/database.sqlite"}

printf "%s[?]%s Read timeout [300]: " "$YELLOW" "$NC"
read read_timeout
read_timeout=${read_timeout:-"300"}

printf "%s[?]%s Handshake timeout [10]: " "$YELLOW" "$NC"
read handshake_timeout
handshake_timeout=${handshake_timeout:-"10"}

print_message "Setting up rights to reading .cert and .key files for $APP_USER..."
grant_cert_access "$tls_cert"
grant_cert_access "$tls_key"
print_message "Right to read .cert and .key successfully granded"

print_message "Config generating... $ENV_FILE..."
cat <<EOF > "$ENV_FILE"
SERVER_ADDRESS=$server_addr
DB_PATH=$db_path
READ_TIMEOUT=$read_timeout
HANDSHAKE_TIMEOUT=$handshake_timeout
TLS_CERT_PATH=$tls_cert
TLS_KEY_PATH=$tls_key
HOME=$DATA_DIR
XDG_DATA_HOME=$DATA_DIR
EOF
chmod 640 "$ENV_FILE"
chown root:"$APP_USER" "$ENV_FILE"

print_message "Donwloading latest release of server from GitHub..."
if ! command -v wget &> /dev/null; then
    print_error "`wget` package not found. Installing with `apt install wget`"
    apt install wget
fi

wget -qO /tmp/muc-server.tar.gz "$DOWNLOAD_URL"
if [ $? -ne 0 ]; then
    print_error "Error while downloading. Check URL: $DOWNLOAD_URL"
    exit 1
fi

tar -xzf /tmp/muc-server.tar.gz -C "$INSTALL_DIR"
rm /tmp/muc-server.tar.gz

BIN_FILE="$INSTALL_DIR/MUC-server"
if [ ! -f "$BIN_FILE" ]; then
    BIN_FILE=$(find "$INSTALL_DIR" -type f -name "MUC-server" | head -n 1)
fi

if [ -z "$BIN_FILE" ]; then
    print_error "MUC-server binary file not found!"
    exit 1
fi

chmod +x "$BIN_FILE"
chown -R "$APP_USER":"$APP_USER" "$INSTALL_DIR"
chown -R "$APP_USER":"$APP_USER" "$DATA_DIR"

print_message "Creating systemd service..."
cat <<EOF > "$SERVICE_FILE"
[Unit]
Description=MUC Server
After=network.target

[Service]
Type=simple
User=$APP_USER
Group=$APP_USER
EnvironmentFile=$ENV_FILE
ExecStart=$BIN_FILE
Restart=on-failure
RestartSec=5s

NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$DATA_DIR

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable muc-server
systemctl start muc-server

print_message "Installing complete!"
echo -e "${CYAN}========================================${NC}"
systemctl status muc-server --no-pager
echo -e "${CYAN}========================================${NC}"
print_warning "If service not launch, check logs: journalctl -u muc-server -e"
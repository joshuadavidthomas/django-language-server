FROM node:20-slim

# Install dependencies
RUN apt-get update && apt-get install -y \
    python3 \
    python3-pip \
    git \
    && rm -rf /var/lib/apt/lists/*

# Install Django Language Server
RUN pip3 install django-language-server

# Create app directory
WORKDIR /app

# Copy package.json and package-lock.json
COPY package*.json ./

# Install dependencies
RUN npm install

# Copy the rest of the code
COPY . .

# Compile TypeScript
RUN npm run compile

# Package the extension
RUN npm run package

# Install code-server
RUN npm install -g code-server

# Create a test project
RUN chmod +x ./create-test-project.sh && ./create-test-project.sh

# Install the extension in code-server
RUN code-server --install-extension ./vscode-django-language-server-0.1.0.vsix

# Expose port
EXPOSE 12000

# Start code-server
CMD ["code-server", "--bind-addr", "0.0.0.0:12000", "--auth", "none", "/app/test-django-project"]
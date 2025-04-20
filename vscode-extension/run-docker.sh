#!/bin/bash

# Run the extension in a Docker container
echo "Running Django Language Server VS Code Extension in a Docker container..."

# Check if Docker is installed
if ! command -v docker &> /dev/null; then
    echo "Docker not found. Please install Docker and try again."
    exit 1
fi

# Check if Docker Compose is installed
if ! command -v docker-compose &> /dev/null; then
    echo "Docker Compose not found. Please install Docker Compose and try again."
    exit 1
fi

# Build and start the Docker container
echo "Building and starting the Docker container..."
docker-compose up --build

echo "Docker session ended."
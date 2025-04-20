# Contributing to the Django Language Server VS Code Extension

Thank you for your interest in contributing to the Django Language Server VS Code extension! This document provides guidelines and instructions for contributing.

## Development Setup

1. Clone the repository:
   ```bash
   git clone https://github.com/your-username/vscode-django-language-server.git
   cd vscode-django-language-server
   ```

2. Install dependencies:
   ```bash
   npm install
   ```

3. Compile the TypeScript code:
   ```bash
   npm run compile
   ```

4. Open the project in VS Code:
   ```bash
   code .
   ```

5. Press F5 to launch the extension in debug mode.

## Project Structure

- `src/extension.ts`: Main extension entry point
- `package.json`: Extension manifest
- `language-configuration.json`: Language configuration for Django HTML

## Making Changes

1. Create a new branch for your changes:
   ```bash
   git checkout -b feature/your-feature-name
   ```

2. Make your changes to the code.

3. Compile the TypeScript code:
   ```bash
   npm run compile
   ```

4. Test your changes by pressing F5 in VS Code to launch the extension in debug mode.

5. Commit your changes:
   ```bash
   git commit -am "Add your feature description"
   ```

6. Push your changes to your fork:
   ```bash
   git push origin feature/your-feature-name
   ```

7. Create a pull request.

## Packaging the Extension

To package the extension for distribution:

```bash
npm install -g @vscode/vsce
vsce package
```

This will create a `.vsix` file that can be installed in VS Code.

## Reporting Issues

If you encounter any issues or have suggestions for improvements, please open an issue on the GitHub repository.

## Code Style

- Follow the existing code style in the project.
- Use meaningful variable and function names.
- Add comments for complex logic.
- Write clear commit messages.

Thank you for your contributions!
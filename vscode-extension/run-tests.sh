#!/bin/bash

# Run the tests
echo "Running Django Language Server VS Code Extension tests..."

# Compile TypeScript
npm run compile

# Check if the test directory exists
if [ ! -d "src/test" ]; then
    echo "Test directory not found. Creating..."
    mkdir -p src/test
fi

# Check if the test file exists
if [ ! -f "src/test/extension.test.ts" ]; then
    echo "Test file not found. Creating..."
    cat > src/test/extension.test.ts << EOF
import * as assert from 'assert';
import * as vscode from 'vscode';

suite('Extension Test Suite', () => {
    vscode.window.showInformationMessage('Start all tests.');

    test('Extension should be present', () => {
        assert.ok(vscode.extensions.getExtension('django-language-server.vscode-django-language-server'));
    });

    test('Extension should activate', async () => {
        const extension = vscode.extensions.getExtension('django-language-server.vscode-django-language-server');
        if (!extension) {
            assert.fail('Extension not found');
            return;
        }
        
        await extension.activate();
        assert.ok(extension.isActive);
    });
});
EOF
fi

# Check if the test runner file exists
if [ ! -f "src/test/runTest.ts" ]; then
    echo "Test runner file not found. Creating..."
    cat > src/test/runTest.ts << EOF
import * as path from 'path';
import { runTests } from '@vscode/test-electron';

async function main() {
    try {
        // The folder containing the Extension Manifest package.json
        // Passed to \`--extensionDevelopmentPath\`
        const extensionDevelopmentPath = path.resolve(__dirname, '../../');

        // The path to the extension test script
        // Passed to --extensionTestsPath
        const extensionTestsPath = path.resolve(__dirname, './extension.test');

        // Download VS Code, unzip it and run the integration test
        await runTests({ extensionDevelopmentPath, extensionTestsPath });
    } catch (err) {
        console.error('Failed to run tests', err);
        process.exit(1);
    }
}

main();
EOF
fi

# Check if the test index file exists
if [ ! -f "src/test/index.ts" ]; then
    echo "Test index file not found. Creating..."
    cat > src/test/index.ts << EOF
import * as path from 'path';
import * as Mocha from 'mocha';
import * as glob from 'glob';

export function run(): Promise<void> {
    // Create the mocha test
    const mocha = new Mocha({
        ui: 'tdd',
        color: true
    });

    const testsRoot = path.resolve(__dirname, '..');

    return new Promise((c, e) => {
        glob('**/**.test.js', { cwd: testsRoot }, (err, files) => {
            if (err) {
                return e(err);
            }

            // Add files to the test suite
            files.forEach(f => mocha.addFile(path.resolve(testsRoot, f)));

            try {
                // Run the mocha test
                mocha.run(failures => {
                    if (failures > 0) {
                        e(new Error(\`\${failures} tests failed.\`));
                    } else {
                        c();
                    }
                });
            } catch (err) {
                e(err);
            }
        });
    });
}
EOF
fi

# Install test dependencies
echo "Installing test dependencies..."
npm install --save-dev @vscode/test-electron mocha @types/mocha glob @types/glob

# Update package.json to include test script
if ! grep -q "\"test\":" package.json; then
    sed -i '/\"lint\":/a\\    \"test\": \"node ./out/test/runTest.js\",' package.json
fi

# Run the tests
echo "Running tests..."
npm test

echo "Tests completed!"
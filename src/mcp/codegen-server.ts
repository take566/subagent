/**
 * CodeGen Agent MCP Server
 */

import { MCPServer } from './server.js';
import { CodeGenAgent } from '../agents/codegen/codegen-agent.js';

const agent = new CodeGenAgent();
const server = new MCPServer(agent);

server.start().catch((error) => {
  console.error('Failed to start CodeGen MCP Server:', error);
  process.exit(1);
});


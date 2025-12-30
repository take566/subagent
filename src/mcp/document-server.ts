/**
 * Document Agent MCP Server
 */

import { MCPServer } from './server.js';
import { DocumentAgent } from '../agents/document/document-agent.js';

const agent = new DocumentAgent();
const server = new MCPServer(agent);

server.start().catch((error) => {
  console.error('Failed to start Document MCP Server:', error);
  process.exit(1);
});


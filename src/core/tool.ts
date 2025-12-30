/**
 * ツールシステム
 */

import type { Tool } from '../types/index.js';

export class ToolRegistry {
  private tools: Map<string, Tool> = new Map();

  /**
   * ツールを登録
   */
  register(tool: Tool): void {
    this.tools.set(tool.name, tool);
  }

  /**
   * ツールを取得
   */
  get(name: string): Tool | undefined {
    return this.tools.get(name);
  }

  /**
   * ツールが存在するか確認
   */
  has(name: string): boolean {
    return this.tools.has(name);
  }

  /**
   * すべてのツールを取得
   */
  getAll(): Tool[] {
    return Array.from(this.tools.values());
  }

  /**
   * ツール名のリストを取得
   */
  getNames(): string[] {
    return Array.from(this.tools.keys());
  }
}

/**
 * ベースツール実装
 */
export abstract class BaseTool implements Tool {
  constructor(
    public name: string,
    public description: string,
    public parameters: Record<string, any>
  ) {}

  abstract invoke(params: Record<string, any>): Promise<any>;
}


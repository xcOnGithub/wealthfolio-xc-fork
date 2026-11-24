import type { BacktestRequest, BacktestResponse } from "@/lib/types/backtest";

import { invoke } from "./platform";

export const runBacktest = async (request: BacktestRequest): Promise<BacktestResponse> => {
  return invoke<BacktestResponse>("run_backtest", { request });
};

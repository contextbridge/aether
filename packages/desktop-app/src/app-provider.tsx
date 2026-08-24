import {
  createContext,
  useContext,
  useState,
  type PropsWithChildren,
} from "react";
import { useStore } from "zustand";
import { AppActions } from "./app-actions";
import { createChatStore, type ChatState, type ChatStore } from "./acp-store";

export type AppServices = {
  store: ChatStore;
  actions: AppActions;
};

export const createAppServices = (): AppServices => {
  const store = createChatStore();
  return { store, actions: new AppActions(store) };
};

const AppContext = createContext<AppServices | null>(null);

export function AppProvider({
  children,
  services,
}: PropsWithChildren<{ services?: AppServices }>) {
  const [value] = useState(() => services ?? createAppServices());
  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}

export function useChatStore<T>(selector: (state: ChatState) => T): T {
  return useStore(useAppServices().store, selector);
}

export function useAppActions(): AppActions {
  return useAppServices().actions;
}

const useAppServices = (): AppServices => {
  const services = useContext(AppContext);
  if (services === null) {
    throw new Error("AppProvider is required");
  }
  return services;
};

/** PROTOTYPE — throwaway. Lets node views look up other nodes (e.g. a Note source's notes). */
import { createContext, useContext } from "react";

import type { DocNode } from "../model/document";

const DocNodesContext = createContext<Map<string, DocNode>>(new Map());
export const DocNodesProvider = DocNodesContext.Provider;
export const useDocNodes = () => useContext(DocNodesContext);

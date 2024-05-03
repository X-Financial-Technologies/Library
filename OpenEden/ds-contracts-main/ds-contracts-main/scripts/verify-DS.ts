import { verifyContract } from "./utils"

import * as dotenv from "dotenv";
dotenv.config();

async function main() {

    await verifyContract("DSGuardFactory");
    await verifyContract("DSProxyFactory");
    await verifyContract("ProxyRegistry");
}

main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
});
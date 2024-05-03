import { deployContract } from "./utils"

import * as dotenv from "dotenv";
dotenv.config();

async function main() {

    const dsGuardFactoryContract = await deployContract("DSGuardFactory");
    const dsProxyFactoryContract = await deployContract("DSProxyFactory");
    const proxyRegistryContract = await deployContract("ProxyRegistry", [dsProxyFactoryContract.address]);
}

main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
});